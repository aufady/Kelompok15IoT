# Pengujian Kestabilan Transmisi Data Sensor DHT22 dan Mekanisme Pembaruan  OTA pada Platform ThingsBoard Menggunakan ESP32-S3
Proyek ini mengimplementasikan sistem Internet of Things (IoT) menggunakan mikrokontroler ESP32-S3 dengan bahasa pemrograman Rust Embedded. Sistem berfungsi untuk memantau suhu dan kelembapan secara real-time menggunakan sensor DHT22, mengirimkan data ke platform ThingsBoard Cloud melalui protokol MQTT, serta mendukung pembaruan firmware OTA (Over-The-Air) tanpa perlu koneksi kabel.

## Authors
1. Muhammad Salman Alfarisyi (2042231006)  
2. Muhammad Aufa Affandi (2042231011)  
3. Ahmad Radhy (Supervisor)

Teknik Instrumentasi - Institut Teknologi Sepuluh Nopember Surabaya

## ⚙️ Fitur

1. **Pembacaan Sensor DHT22**
   - Mengukur suhu dan kelembapan secara periodik.
   - Menampilkan hasil di serial monitor serta mengirim ke Thingsboard cloud.

2. **Koneksi MQTT ke ThingsBoard Cloud**
   - Mengirim data telemetry (`temperature`, `humidity`) menggunakan topik terautentikasi.
   - Dapat dipantau melalui dashboard real-time.

3. **Over-The-Air (OTA) Update**
   - Perangkat dapat menerima pembaruan firmware dari server MQTT.
   - Setelah update, sistem menampilkan log *“OTA selesai, restart...”* dan melakukan reboot otomatis.

4. **Analisis Latency dan Stabilitas Data**
   - Data diukur selama 4 hari (9–12 Oktober 2025).
   - Visualisasi dilakukan menggunakan **Gnuplot** untuk membandingkan variasi delay antar-pengiriman data.

---

## 🧩 Kebutuhan Sistem

### 💡 Perangkat Keras
- Mikrokontroler **ESP32-S3**
- Sensor **DHT22 (AM2302)**
- kabel jumper
- **Adaptor 5V / charger HP** sebagai sumber daya
- Komputer dengan koneksi Wi-Fi

### 💻 Perangkat Lunak
- **Rust (toolchain nightly)**
- **esp-idf** & **esp-flash**
- **ThingsBoard Cloud Account**
- **Gnuplot** untuk analisis grafik latency

---

## 🔄 Langkah Penggunaan

### 1️⃣ Clone Repository
```bash
git clone https://github.com/aufady/esp32-rust-ota-thingsboard
cd esp32-rust-ota-thingsboard

### 2️⃣ Siapkan Toolchain
rustup target add xtensa-esp32s3-none-elf
cargo install espflash
cargo install espup
espup install

### 3️⃣ Build 
cargo build

### 4️⃣ Flash Firmware dan Jalankan Server OTA
espflash flash --partition-table partition_table.csv target/xtensa-esp32s3-espidf/debug/dev --monitor --port /dev/ttyACM0
Ketika firmware dikirim, pe akan menampilkan:
Menerima firmware baru...
OTA selesai, restart...

### 5️⃣ Monitoring di ThingsBoard
Buka dashboard di ThingsBoard Cloud.
Lihat perubahan nilai suhu dan kelembapan secara real-time.

### 🗂️ Struktur Proyek
esp32-rust-ota-thingsboard/
├── src/
│   ├── main.rs              # Program utama Rust bare-metal
│   ├── dht22.rs             # Modul pembacaan sensor DHT22
│   ├── mqtt.rs              # Modul koneksi dan publish MQTT
│   ├── ota.rs               # Modul OTA update
│   └── utils.rs             # Fungsi logging dan helper
├── ota_server.py            # Server HTTP untuk OTA update
├── Cargo.toml               # Konfigurasi dependensi Rust
├── README.md                # Dokumentasi proyek
└── results/
    ├── data-log.csv         # Hasil pengujian sensor
    ├── latency.gnuplot      # Script visualisasi latency
    └── grafik-latency.png   # Grafik hasil Gnuplot

### 📊 Hasil Pengujian
Suhu rata-rata: 29–31 °C
Kelembapan rata-rata: 70–74 %
Latency rata-rata: 180 ms
Keberhasilan OTA: 100 % (berhasil)

