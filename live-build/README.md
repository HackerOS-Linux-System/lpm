# MyDistro — live-build + lpm

Własna dystrybucja Debian testing z **lpm** jako domyślnym package managerem.

## Struktura

```
mydistro/
├── auto/
│   ├── config          # lb config wrapper
│   ├── build           # lb build wrapper
│   └── clean           # lb clean wrapper
├── config/
│   ├── hooks/
│   │   ├── normal/
│   │   │   ├── 0100-install-lpm.hook.chroot      # instaluje lpm binarke
│   │   │   ├── 0200-apt-lpm-bridge.hook.chroot   # tworzy apt→lpm wrappery
│   │   │   └── 0300-system-config.hook.chroot    # MOTD, os-release, completion
│   │   └── live/
│   │       └── 0010-lpm-init.hook.live           # init lpm przy starcie live
│   ├── package-lists/
│   │   ├── base.list.chroot      # pakiety bazowe (apt podczas lb build)
│   │   └── desktop.list.chroot  # opcjonalne GUI
│   └── includes.chroot/
│       ├── etc/lpm/
│       │   └── sources-list.toml   # repozytoria lpm
│       └── usr/local/bin/
│           └── lpm                 # ← TU WRZUĆ BINARKE lpm!
└── build.log
```

## Przed budowaniem

### 1. Skompiluj lpm

```bash
cd ~/lpm
cargo build --release
```

### 2. Skopiuj binarke do projektu

```bash
cp ~/lpm/target/release/lpm \
   mydistro/config/includes.chroot/usr/local/bin/lpm
```

### 3. Zainstaluj live-build

```bash
sudo apt install live-build
```

## Budowanie ISO

```bash
cd mydistro

# Wygeneruj konfigurację
sudo lb config

# Zbuduj ISO (wymaga roota, zajmuje ~10-20 min)
sudo lb build
```

Gotowy ISO: `live-image-amd64.hybrid.iso`

## Testowanie w QEMU

```bash
qemu-system-x86_64 \
    -m 2048 \
    -cdrom live-image-amd64.hybrid.iso \
    -boot d \
    -enable-kvm
```

## Testowanie na pendrive

```bash
sudo dd if=live-image-amd64.hybrid.iso of=/dev/sdX bs=4M status=progress
sync
```

## Po uruchomieniu live

```bash
# lpm jest gotowy od razu
lpm update
lpm search firefox
lpm install firefox

# apt również działa (wrapper → lpm)
apt install vim
apt-get update
```

## Czyszczenie

```bash
sudo lb clean          # usuwa chroot i binary
sudo lb clean --purge  # usuwa wszystko łącznie z cache
```

## Customizacja

### Dodaj pakiety do ISO

W `config/package-lists/base.list.chroot`:
```
firefox-esr
thunderbird
vlc
```

### Zmień dystrybucję bazową

W `auto/config` zmień:
```bash
--distribution testing
```
na `bookworm`, `trixie`, `forky` itp.

### Własne repo w lpm

W `config/includes.chroot/etc/lpm/sources-list.toml` dodaj:
```toml
[[repo]]
name    = "moje-repo"
enabled = true
baseurl = "http://packages.mojadystrybucja.pl/debian"
suite   = "stable"
components = ["main"]
arch    = ["amd64"]
```
