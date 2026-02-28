#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════
# Unified VM Script - Works with or without Nix
# ═══════════════════════════════════════════════════════════════════════════

# ── Nix Integration ────────────────────────────────────────────────────────
# If Nix is available and we're not already in a nix-shell, re-execute with dependencies
if [ -z "${IN_NIX_SHELL:-}" ] && command -v nix &> /dev/null; then
  if [ -f "$(dirname "$0")/flake.nix" ]; then
    echo "🔧 Loading dependencies via Nix..."
    exec nix develop "$(dirname "$0")#vm" --command "$0" "$@"
  fi
fi

# ── Dependency Check ────────────────────────────────────────────────────────
check_deps() {
  local missing=()
  for cmd in qemu-system-x86_64 qemu-img curl ssh scp; do
    command -v "$cmd" &> /dev/null || missing+=("$cmd")
  done

  if [ "$DISTRO" == "fedora" ] && ! command -v genisoimage &> /dev/null; then
    missing+=("genisoimage")
  fi

  if [ ${#missing[@]} -gt 0 ]; then
    echo "❌ Missing dependencies: ${missing[*]}"
    echo ""
    echo "Install them with:"
    echo "  • Nix: nix develop .#vm"
    echo "  • Debian/Ubuntu: sudo apt install qemu-system-x86 qemu-utils curl openssh-client genisoimage"
    echo "  • Fedora: sudo dnf install qemu curl openssh-clients genisoimage"
    echo "  • Arch: sudo pacman -S qemu-full curl openssh cdrtools"
    exit 1
  fi
}

# ── Distro Detection & Config ──────────────────────────────────────────────
DISTRO="${1:-}"
if [ -z "$DISTRO" ] || [ "$DISTRO" == "help" ]; then
  cat << EOF
Usage: $(basename "$0") <distro> <command>

Distros:
  debian      Debian 12 (Bookworm) cloud image
  fedora      Fedora 41 cloud image

Commands:
  start       Download image (once) and start VM
  ssh         SSH into the VM
  send FILE [DEST]  Copy file/dir into VM
  stop        Stop the VM
  reset       Delete disk, keep base image (fresh start)
  status      Show if VM is running
  help        Show this help

Examples:
  $(basename "$0") debian start
  $(basename "$0") fedora ssh
  $(basename "$0") debian send myfile.txt /tmp/
EOF
  exit 0
fi

case "$DISTRO" in
  debian)
    VM_DIR="$HOME/.vms/debian"
    SSH_PORT="2222"
    MEMORY="2048"
    CORES="2"
    DISK_SIZE="10G"
    BASE_URL="https://cloud.debian.org/images/cloud/bookworm/latest"
    IMAGE_NAME="debian-12-genericcloud-amd64.qcow2"
    NEEDS_SEED=true
    SSH_USER="debian"
    SSH_PASS="debian"
    ;;
  fedora)
    VM_DIR="$HOME/.vms/fedora"
    SSH_PORT="2223"
    MEMORY="2048"
    CORES="2"
    DISK_SIZE="10G"
    FEDORA_VERSION="41"
    FEDORA_RELEASE="41-1.4"
    BASE_URL="https://download.fedoraproject.org/pub/fedora/linux/releases/${FEDORA_VERSION}/Cloud/x86_64/images"
    IMAGE_NAME="Fedora-Cloud-Base-Generic-${FEDORA_RELEASE}.x86_64.qcow2"
    NEEDS_SEED=true
    SSH_USER="root"
    SSH_PASS="fedora"
    ;;
  *)
    echo "❌ Unknown distro: $DISTRO"
    echo "Available: debian, fedora"
    exit 1
    ;;
esac

shift  # Remove distro argument

BASE_IMAGE="$VM_DIR/base.qcow2"
DISK="$VM_DIR/disk.qcow2"
SEED="$VM_DIR/seed.iso"
PIDFILE="$VM_DIR/vm.pid"

# ── Commands ────────────────────────────────────────────────────────────────
cmd_help() {
  cat << EOF
Usage: $(basename "$0") $DISTRO <command>

Commands:
  start       Download image (once) and start VM
  ssh         SSH into the VM ($SSH_USER)
  send FILE [DEST]  Copy file/dir into VM (default: /root/)
  stop        Stop the VM
  reset       Delete disk, keep base image (fresh start)
  status      Show if VM is running
  help        Show this help
EOF
}

_make_seed() {
  echo "Building cloud-init seed ISO..."
  local seed_dir
  seed_dir=$(mktemp -d)
  trap "rm -rf $seed_dir" RETURN

  cat > "$seed_dir/meta-data" << EOF
instance-id: ${DISTRO}-local
local-hostname: ${DISTRO}-vm
EOF

  cat > "$seed_dir/user-data" << EOF
#cloud-config
users:
  - name: ${SSH_USER}
    groups: sudo
    shell: /bin/bash
    sudo: ['ALL=(ALL) NOPASSWD:ALL']
    lock_passwd: false
ssh_pwauth: true
chpasswd:
  list: |
    ${SSH_USER}:${SSH_PASS}
  expire: false
disable_root: false
package_update: false
package_upgrade: false
runcmd:
  - systemctl enable --now ssh
EOF

  genisoimage \
    -output "$SEED" \
    -volid cidata \
    -joliet -rock \
    "$seed_dir/user-data" "$seed_dir/meta-data"

  echo "Seed ISO created."
}

cmd_start() {
  check_deps
  mkdir -p "$VM_DIR"

  if [ ! -f "$BASE_IMAGE" ]; then
    echo "Downloading $DISTRO cloud image..."
    curl -k -L --progress-bar "$BASE_URL/$IMAGE_NAME" -o "$BASE_IMAGE"
  fi

  if [ "$NEEDS_SEED" == "true" ] && [ ! -f "$SEED" ]; then
    _make_seed
  fi

  if [ ! -f "$DISK" ]; then
    echo "Creating ${DISK_SIZE} COW disk..."
    qemu-img create -f qcow2 -b "$BASE_IMAGE" -F qcow2 "$DISK" "$DISK_SIZE"
  fi

  if [ -f "$PIDFILE" ] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
    echo "VM is already running (PID $(cat "$PIDFILE"))"
    return
  fi

  echo "Starting $DISTRO VM (SSH on port $SSH_PORT)..."

  local qemu_args=(
    -name "${DISTRO}-vm"
    -enable-kvm
    -cpu host
    -m "$MEMORY"
    -smp "$CORES"
    -drive "file=$DISK,format=qcow2,if=virtio,cache=writeback"
    -boot menu=off,strict=on
  )

  if [ "$NEEDS_SEED" == "true" ]; then
    qemu_args+=(-drive "file=$SEED,format=raw,media=cdrom,readonly=on")
  fi

  qemu_args+=(
    -netdev "user,id=net0,hostfwd=tcp::$SSH_PORT-:22"
    -device "virtio-net-pci,netdev=net0"
    -object "rng-random,id=rng0,filename=/dev/urandom"
    -device "virtio-rng-pci,rng=rng0,max-bytes=1024,period=1000"
    -display none
    -serial stdio
    -pidfile "$PIDFILE"
  )

  qemu-system-x86_64 "${qemu_args[@]}" &

  echo ""
  local wait_time="15s"
  [ "$NEEDS_SEED" == "true" ] && wait_time="40s"
  echo "VM started! Wait ~$wait_time then run: $(basename "$0") $DISTRO ssh"
  echo "  user: $SSH_USER  password: $SSH_PASS"
}

cmd_ssh() {
  ssh -p "$SSH_PORT" \
    -o StrictHostKeyChecking=no \
    -o UserKnownHostsFile=/dev/null \
    "$SSH_USER@localhost" "$@"
}

cmd_send() {
  local file="${1:?Usage: $(basename "$0") $DISTRO send <file> [destination]}"
  local dest="${2:-/root/}"
  scp -P "$SSH_PORT" \
    -o StrictHostKeyChecking=no \
    -o UserKnownHostsFile=/dev/null \
    -r "$file" "$SSH_USER@localhost:$dest"
}

cmd_stop() {
  if [ -f "$PIDFILE" ]; then
    local pid
    pid=$(cat "$PIDFILE")
    kill "$pid" 2>/dev/null && echo "VM stopped (PID $pid)." || echo "VM already stopped."
    rm -f "$PIDFILE"
  else
    pkill -f "qemu-system-x86_64.*${DISTRO}" 2>/dev/null && echo "VM stopped." || echo "No VM found."
  fi
}

cmd_reset() {
  cmd_stop 2>/dev/null || true
  rm -f "$DISK"
  [ "$NEEDS_SEED" == "true" ] && rm -f "$SEED"
  echo "Disk wiped. Run '$(basename "$0") $DISTRO start' for a fresh VM."
}

cmd_status() {
  if [ -f "$PIDFILE" ] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
    echo "VM is running (PID $(cat "$PIDFILE"), SSH port $SSH_PORT)"
  else
    echo "VM is not running."
  fi
}

# ── Main ────────────────────────────────────────────────────────────────────
case "${1:-help}" in
  start)  cmd_start ;;
  ssh)    shift; cmd_ssh "$@" ;;
  send)   shift; cmd_send "$@" ;;
  stop)   cmd_stop ;;
  reset)  cmd_reset ;;
  status) cmd_status ;;
  *)      cmd_help ;;
esac
