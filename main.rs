// =========================================================================================
// #![no_std] Architecture Implementation
// Aplikasi ini menggunakan arsitektur 'no_std' dengan memanfaatkan modul 'core'
// dan 'alloc' (untuk alokasi heap) sebagai pengganti Standard Library (std).
// Komponen network/OS disediakan oleh esp-idf-svc, yang berjalan di atas FreeRTOS.
// =========================================================================================

use core::{
    time::Duration, 
    str, // Menggantikan std::str
    sync::atomic::{AtomicBool, Ordering}, // Menggantikan std::sync::atomic
};

// Menggunakan alokasi heap dari modul 'alloc'
extern crate alloc;
use alloc::sync::Arc;

// Di lingkungan ESP-IDF Rust, 'thread' adalah wrapper ergonomis yang membuat FreeRTOS Task.
// Ini adalah cara idiomatik untuk membuat Task baru di embedded Rust dengan ESP-IDF.
use thread; 

use anyhow::{Result, Error};
// Chrono tidak memiliki versi no_std murni, tetapi berfungsi baik di environment ESP-IDF.
use chrono::{Duration as ChronoDuration, NaiveDateTime, Utc, TimeZone}; 
use dht_sensor::dht22::Reading;
use dht_sensor::DhtReading;
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::{
        delay::Ets,
        // Driver pin yang eksplisit untuk interaksi perangkat keras langsung
        gpio::PinDriver, 
        prelude::*,
    },
    log::EspLogger,
    mqtt::client::*,
    nvs::EspDefaultNvsPartition,
    sntp,
    systime::EspSystemTime,
    wifi::*,
    ota::EspOta,
    http::client::EspHttpConnection,
};
use embedded_svc::{
    mqtt::client::QoS,
    http::client::Client, 
    io::Read, 
};
use heapless::String; // String yang ramah 'no_std'
use serde_json::json;

// --- Konfigurasi Firmware & Device ---
const CURRENT_FIRMWARE_VERSION: &str = "PaceP-s3-v2.0";
const TB_MQTT_URL: &str = "mqtt://mqtt.thingsboard.cloud:1883";
const THINGSBOARD_TOKEN: &str = "3IHhjZZYoPP1P2Mu0V61";

// --- FUNGSI C (Interaksi Sistem Langsung) ---
// Deklarasi fungsi C/ESP-IDF untuk me-restart sistem secara low-level.
extern "C" {
    fn esp_restart();
}

// =========================================================================================
// --- MQTT Client State (Global Access) ---
// Digunakan untuk memungkinkan Task/Handler lain (seperti OTA Task) mengirim pesan.
static mut MQTT_CLIENT: Option<EspMqttClient<'static>> = None;

fn get_mqtt_client() -> Option<&'static mut EspMqttClient<'static>> {
    unsafe {
        MQTT_CLIENT.as_mut().map(|c| {
            // SAFETY: Transmute diperlukan karena callback C tidak tahu tentang lifetime Rust
            core::mem::transmute::<&mut EspMqttClient<'_>, &mut EspMqttClient<'static>>(c)
        })
    }
}

// -----------------------------------------------------------------------------------------
// --- PUBLISH FUNGSI (Memanfaatkan Global Client) ---

// Fungsi untuk mengirim telemetry fw_state
fn publish_fw_state(state: &str) {
    let payload = format!("{{\"fw_state\":\"{}\"}}", state);
    log::info!("➡ Mengirim telemetry fw_state: {}", payload);

    if let Some(client) = get_mqtt_client() {
        if let Err(e) = client.publish(
            "v1/devices/me/telemetry",
            QoS::AtLeastOnce, // QoS 1: Memastikan pengiriman ke ThingsBoard
            false,
            payload.as_bytes(),
        ) {
            log::error!("⚠ Gagal kirim fw_state {}: {:?}", state, e);
        }
    } else {
        log::error!("⚠ MQTT client belum siap untuk kirim fw_state {}", state);
    }
}

// Mengirim versi firmware saat ini
fn publish_fw_version() {
    let payload = format!("{{\"fw_version\":\"{}\"}}", CURRENT_FIRMWARE_VERSION);
    log::info!("➡ Mengirim Current FW Version: {}", payload);

    if let Some(client) = get_mqtt_client() {
        if let Err(e) = client.publish(
            "v1/devices/me/telemetry",
            QoS::AtLeastOnce,
            false,
            payload.as_bytes(),
        ) {
            log::error!("⚠ Gagal kirim fw_version: {:?}", e);
        }
    } else {
        log::error!("⚠ MQTT client belum siap untuk kirim fw_version");
    }
}

// Fungsi untuk mengirim RPC response ke ThingsBoard
fn send_rpc_response(request_id: &str, status: &str) {
    let topic = format!("v1/devices/me/rpc/response/{}", request_id);
    log::info!("➡ Mengirim RPC response ke: {}", topic);

    let payload = format!("{{\"status\":\"{}\"}}", status);

    if let Some(client) = get_mqtt_client() {
        if let Err(e) = client.publish(
            topic.as_str(),
            QoS::AtLeastOnce,
            false,
            payload.as_bytes(),
        ) {
            log::error!("⚠ Gagal kirim RPC response: {:?}", e);
        }
    } else {
        log::error!("⚠ MQTT client belum siap untuk kirim RPC response");
    }
}

// -----------------------------------------------------------------------------------------
// --- OTA PROCESS FUNCTION (Berjalan di FreeRTOS Task terpisah) ---
fn ota_process(url: alloc::string::String) {
    // NOTE: argumen diubah menjadi String (owned) agar dapat dipindah (move) ke Task baru
    log::info!("📥 Mulai OTA dari URL: {}", url);
    publish_fw_state("DOWNLOADING");

    // Jeda singkat untuk memastikan pesan "DOWNLOADING" terkirim oleh Task MQTT
    thread::sleep(Duration::from_millis(500));

    match EspOta::new() {
        Ok(mut ota) => {
            let http_config = esp_idf_svc::http::client::Configuration {
                ..Default::default()
            };

            let conn = match EspHttpConnection::new(&http_config) {
                Ok(c) => c,
                Err(e) => {
                    log::error!("⚠ Gagal buat koneksi HTTP: {:?}", e);
                    publish_fw_state("FAILED");
                    return;
                }
            };

            let mut client = Client::wrap(conn);
            // Menggunakan &url.as_str() karena type data url sekarang adalah String (alloc)
            let request = match client.get(url.as_str()) { 
                Ok(r) => r,
                Err(e) => {
                    log::error!("⚠ Gagal buat HTTP GET: {:?}", e);
                    publish_fw_state("FAILED");
                    return;
                }
            };

            let mut response = match request.submit() {
                Ok(r) => r,
                Err(e) => {
                    log::error!("⚠ Gagal submit request: {:?}", e);
                    publish_fw_state("FAILED");
                    return;
                }
            };

            if response.status() < 200 || response.status() >= 300 {
                log::error!("⚠ HTTP request gagal. Status code: {}", response.status());
                publish_fw_state("FAILED");
                return;
            }

            let mut buf = [0u8; 1024];
            let mut update = match ota.initiate_update() {
                Ok(u) => u,
                Err(e) => {
                    log::error!("⚠ Gagal init OTA: {:?}", e);
                    publish_fw_state("FAILED");
                    return;
                }
            };

            loop {
                match response.read(&mut buf) {
                    Ok(0) => break,
                    Ok(size) => {
                        if let Err(e) = update.write(&buf[..size]) {
                            log::error!("⚠ Gagal tulis OTA: {:?}", e);
                            publish_fw_state("FAILED");
                            return;
                        }
                    }
                    Err(e) => {
                        log::error!("⚠ HTTP read error: {:?}", e);
                        publish_fw_state("FAILED");
                        return;
                    }
                }
            }

            publish_fw_state("VERIFYING");

            if let Err(e) = update.complete() {
                log::error!("⚠ OTA complete error: {:?}", e);
                publish_fw_state("FAILED");
                return;
            }

            log::info!("✅ OTA selesai, restart...");
            publish_fw_state("SUCCESS");

            // Jeda 1 detik agar pesan SUCCESS terkirim sebelum restart
            thread::sleep(Duration::from_secs(1));

            // Panggilan fungsi C untuk restart sistem
            unsafe { esp_restart(); }
        }
        Err(e) => {
            log::error!("⚠ Gagal init OTA: {:?}", e);
            publish_fw_state("FAILED");
        }
    }
}

// -----------------------------------------------------------------------------------------
// --- MAIN APPLICATION (Berjalan di Main Task/Core 1) ---
fn main() -> Result<(), Error> {
    // --- Inisialisasi dasar (Sistem & Log) ---
    esp_idf_svc::sys::link_patches();
    EspLogger::initialize_default();
    log::info!("🚀 Program dimulai, Versi FW: {} - 🔥 FIRMWARE AKTIF!", CURRENT_FIRMWARE_VERSION);

    // --- Inisialisasi perangkat ---
    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take().unwrap();

    // --- Konfigurasi WiFi ---
    let mut wifi = EspWifi::new(peripherals.modem, sysloop.clone(), Some(nvs.clone()))?;

    let mut ssid: String<32> = String::new();
    ssid.push_str("No Internet").unwrap(); // Ganti dengan SSID Anda

    let mut pass: String<64> = String::new();
    pass.push_str("tertolong123").unwrap(); // Ganti dengan Password Anda

    let wifi_config = Configuration::Client(ClientConfiguration {
        ssid,
        password: pass,
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    });

    log::info!("🔗 Koneksi WiFi dimulai...");
    wifi.set_configuration(&wifi_config)?;
    wifi.start()?;
    wifi.connect()?;

    // Tunggu sampai WiFi benar-benar aktif (Proses Blocking)
    while !wifi.is_connected().unwrap() {
        log::info!("⏳ Menunggu koneksi WiFi...");
        thread::sleep(Duration::from_secs(1));
    }
    log::info!("✅ WiFi terhubung!");

    // --- Manajemen Sumber Daya (Pinning Services) ---
    let _wifi = alloc::boxed::Box::leak(alloc::boxed::Box::new(wifi));
    let _sysloop = alloc::boxed::Box::leak(alloc::boxed::Box::new(sysloop));
    let _nvs = alloc::boxed::Box::leak(alloc::boxed::Box::new(nvs));

    // --- Sinkronisasi waktu via NTP ---
    log::info!("🌐 Sinkronisasi waktu NTP...");
    let sntp = sntp::EspSntp::new_default()?;

    // Tunggu sinkronisasi NTP (Proses Blocking)
    loop {
        if sntp.get_sync_status() == sntp::SyncStatus::Completed {
            log::info!("✅ Waktu berhasil disinkronkan dari NTP");
            break;
        }
        log::info!("⏳ Menunggu sinkronisasi NTP...");
        thread::sleep(Duration::from_secs(1));
    }

    // Delay tambahan agar waktu stabil
    thread::sleep(Duration::from_secs(5));

    // --- Konfigurasi MQTT (ThingsBoard Cloud) ---
    let mqtt_config = MqttClientConfiguration {
        client_id: Some("esp32-rust-ota"),
        username: Some(THINGSBOARD_TOKEN),
        password: None,
        keep_alive_interval: Some(Duration::from_secs(30)),
        ..Default::default()
    };

    let mqtt_connected = Arc::new(AtomicBool::new(false));

    // --- MQTT Callback Handler (Event-Driven) ---
    let mqtt_callback = {
        let mqtt_connected = mqtt_connected.clone();

        move |event: EspMqttEvent<'_>| {
            use esp_idf_svc::mqtt::client::EventPayload;

            match event.payload() {
                EventPayload::Connected(_) => {
                    log::info!("📡 MQTT connected");
                    mqtt_connected.store(true, Ordering::SeqCst);
                }
                EventPayload::Received { topic, data, .. } => {
                    // Menggunakan core::str::from_utf8
                    let payload_str = str::from_utf8(data).unwrap_or(""); 
                    log::info!("📩 Payload diterima. Topic: {:?}, Data: {}", topic, payload_str);

                    if let Some(topic_str) = topic {
                        if topic_str.starts_with("v1/devices/me/rpc/request/") {
                            let parts: alloc::vec::Vec<&str> = topic_str.split('/').collect();
                            if let Some(request_id) = parts.last() {
                                log::info!("✅ Menerima RPC request_id: {}", request_id);

                                if let Ok(json) = serde_json::from_str::<serde_json::Value>(payload_str) {
                                    
                                    let ota_url_owned = json.get("params")
                                                            .and_then(|p| p.get("ota_url"))
                                                            .and_then(|u| u.as_str())
                                                            // Menggunakan alloc::string::ToString untuk cloning
                                                            .map(|s| s.to_string()); 

                                    if let Some(url) = ota_url_owned {
                                        log::info!("⚡ Dapat OTA URL dari RPC: {}", url);
                                        send_rpc_response(request_id, "success");
                                        
                                        // Pembuatan Task (Thread) dengan stack size eksplisit
                                        thread::Builder::new()
                                            .name("ota_task")
                                            .stack_size(10 * 1024) 
                                            .spawn(move || {
                                                ota_process(url); // Menerima String yang di-move
                                            })
                                            .expect("Gagal membuat FreeRTOS Task OTA");
                                        
                                        return;
                                    } else {
                                        log::warn!("⚠ Payload RPC diterima, tetapi \"ota_url\" tidak ditemukan.");
                                    }
                                } else {
                                    log::error!("⚠ Gagal mem-parse JSON payload RPC.");
                                }
                                send_rpc_response(request_id, "failure");
                            }
                        }
                    }
                }
                EventPayload::Disconnected => {
                    log::warn!("⚠ MQTT Disconnected!");
                    mqtt_connected.store(false, Ordering::SeqCst);
                }
                _ => {}
            }
        }
    };

    // --- Inisialisasi MQTT Client ---
    let client = loop {
        let res = unsafe {
            EspMqttClient::new_nonstatic_cb(
                TB_MQTT_URL,
                &mqtt_config,
                mqtt_callback.clone(),
            )
        };

        match res {
            Ok(c) => {
                unsafe { MQTT_CLIENT = Some(c) };

                if let Some(c_ref) = get_mqtt_client() {
                    while !mqtt_connected.load(Ordering::SeqCst) {
                        log::info!("⏳ Menunggu MQTT connect...");
                        thread::sleep(Duration::from_millis(500));
                    }
                    log::info!("📡 MQTT Connected!");

                    c_ref.subscribe("v1/devices/me/rpc/request/+", QoS::AtLeastOnce).unwrap();

                    publish_fw_version();
                    publish_fw_state("IDLE");

                    break c_ref;
                } else {
                    log::error!("⚠ Gagal mendapatkan referensi client setelah koneksi.");
                    thread::sleep(Duration::from_secs(5));
                    continue;
                }
            }
            Err(e) => {
                log::error!("⚠ MQTT connect gagal: {:?}", e);
                thread::sleep(Duration::from_secs(5));
            }
        }
    };

    // --- Inisialisasi sensor DHT22 (GPIO4) ---
    let mut pin = PinDriver::input_output_od(peripherals.pins.gpio4)?;
    let mut delay = Ets; 

    // --- Loop utama kirim data (Core Task) ---
    loop {
        // Ambil waktu sekarang dari SystemTime
        let systime = EspSystemTime {}.now();
        let secs = systime.as_secs() as i64;
        let nanos = systime.subsec_nanos();
        
        let naive = NaiveDateTime::from_timestamp_opt(secs, nanos as u32)
            .unwrap_or(NaiveDateTime::from_timestamp_opt(0, 0).unwrap());
        
        // Konversi ke WIB (UTC + 7 jam)
        let utc_time = Utc.from_utc_datetime(&naive);
        let wib_time = utc_time + ChronoDuration::hours(7);
        let ts_millis = naive.and_utc().timestamp_millis();
        let send_time_str = wib_time.format("%Y-%m-%d %H:%M:%S").to_string();

        // Baca sensor DHT22
        match Reading::read(&mut delay, &mut pin) {
            Ok(Reading {
                temperature,
                relative_humidity,
            }) => {
                // Siapkan payload JSON
                let payload = json!({
                    "send_time": send_time_str,
                    "ts": ts_millis,
                    "temperature": temperature,
                    "humidity": relative_humidity
                });

                let payload_str = payload.to_string();

                // Kirim data Telemetry
                match client.publish( 
                    "v1/devices/me/telemetry",
                    QoS::AtLeastOnce, 
                    false,
                    payload_str.as_bytes(),
                ) {
                    Ok(_) => log::info!("📤 Data terkirim (T: {}°C, H: {}%): {}", temperature, relative_humidity, payload_str),
                    Err(e) => log::error!("❌ Gagal publish ke MQTT: {:?}", e),
                }
            }
            Err(e) => log::error!("⚠ Gagal baca DHT22: {:?}", e),
        }

        // Delay 60 detik (mengosongkan CPU untuk Task lain)
        thread::sleep(Duration::from_secs(60));
    }
}
