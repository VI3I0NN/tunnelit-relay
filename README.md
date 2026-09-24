# tunnelit-relay

Relay-сервер для туннельного сервиса **tunnelit** — self-hosted аналог playit.gg. Принимает подключения от агентов через WebSocket и проксирует TCP/UDP трафик, позволяя пробрасывать локальные серверы в интернет без port forwarding.

## Возможности

- 🔌 **TCP + UDP** туннелирование через WebSocket
- 🌐 **Веб-панель** для управления туннелями и заявками на субдомены
- 💾 **SQLite** для хранения данных
- 🎯 **Динамические порты** из настраиваемого диапазона (или фиксированный порт)
- 📋 **Workflow субдоменов** — пользователь запрашивает, админ одобряет

## Быстрый старт (для разработки)

```bash
# Склонировать и собрать
git clone https://github.com/VI3I0NN/tunnelit-relay.git
cd tunnelit-relay
cargo build --release

# Запустить
./target/release/tunnelit-relay \
  --ws-port 9090 \
  --http-port 8080 \
  --port-range-start 10000 \
  --port-range-end 60000 \
  --db-path /var/lib/tunnelit/tunnelit.db
```

## CLI-аргументы

| Аргумент | По умолчанию | Описание |
|---|---|---|
| `--ws-port` | `9090` | Порт WebSocket сервера |
| `--http-port` | `8080` | Порт HTTP админ-панели |
| `--port-range-start` | `10000` | Начало диапазона портов для туннелей |
| `--port-range-end` | `60000` | Конец диапазона портов для туннелей |
| `--db-path` | `tunnelit.db` | Путь к SQLite базе данных |

## Деплой на сервер (Debian/Ubuntu)

### 1. Установить Rust и собрать

```bash
# Установить зависимости
sudo apt update && sudo apt install -y build-essential pkg-config curl git

# Установить Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"

# Склонировать и собрать
git clone https://github.com/visionn1488/tunnelit-relay.git
cd tunnelit-relay
cargo build --release

# Скопировать бинарник
sudo cp target/release/tunnelit-relay /usr/local/bin/
```

### 2. Создать systemd-сервис

```bash
# Создать пользователя
sudo useradd -r -s /bin/false tunnelit

# Создать директорию для данных
sudo mkdir -p /var/lib/tunnelit
sudo chown tunnelit:tunnelit /var/lib/tunnelit

# Создать сервис
sudo tee /etc/systemd/system/tunnelit-relay.service << 'EOF'
[Unit]
Description=Tunnelit Relay Server
After=network.target

[Service]
Type=simple
User=tunnelit
Group=tunnelit
ExecStart=/usr/local/bin/tunnelit-relay \
  --ws-port 9090 \
  --http-port 8080 \
  --port-range-start 10000 \
  --port-range-end 60000 \
  --db-path /var/lib/tunnelit/tunnelit.db
Restart=always
RestartSec=5
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
EOF

# Запустить
sudo systemctl daemon-reload
sudo systemctl enable tunnelit-relay
sudo systemctl start tunnelit-relay

# Проверить статус
sudo systemctl status tunnelit-relay
```

### 3. Настроить Caddy

```bash
# Установить Caddy
sudo apt install -y debian-keyring debian-archive-keyring apt-transport-https
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | sudo gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' | sudo tee /etc/apt/sources.list.d/caddy-stable.list
sudo apt update && sudo apt install -y caddy
```

Caddyfile (`/etc/caddy/Caddyfile`):

```caddyfile
# Админ-панель
admin.your.domain.com {
    reverse_proxy localhost:8080
}

# WebSocket для агентов
ws.your.domain.com {
    reverse_proxy localhost:9090
}
```

```bash
sudo systemctl restart caddy
```

### 4. Настроить DNS

В панели управления доменом `your-domain.com` добавить:

| Тип | Имя | Значение |
|---|---|---|
| A | `admin` | `IP_ВАШЕГО_СЕРВЕРА` |
| A | `ws` | `IP_ВАШЕГО_СЕРВЕРА` |
| A | `*` | `IP_ВАШЕГО_СЕРВЕРА` (для субдоменов туннелей) |

### 5. Открыть порты (firewall)

```bash
sudo ufw allow 80/tcp    # Caddy HTTP
sudo ufw allow 443/tcp   # Caddy HTTPS
sudo ufw allow 10000:60000/tcp  # Туннели TCP
sudo ufw allow 10000:60000/udp  # Туннели UDP
```

## Архитектура

```
Клиент (игрок) ──TCP/UDP──► relay:ПОРТ ──WebSocket──► агент ──► локальный сервер
Админ (браузер) ──HTTPS──► Caddy ──► relay:8080
Агент ──WSS──► Caddy ──► relay:9090
```

## Админ-панель

Открыть `https://admin.your-domain.com` — тёмная веб-панель с:
- Таблицей активных туннелей
- Списком заявок на субдомены (Approve / Reject)
- Автообновлением каждые 5 секунд

## Лицензия

MIT
