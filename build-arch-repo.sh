#!/usr/bin/env bash
set -euo pipefail

# Build Arch packages and create repository database
REPO_NAME="plentysound"
REPO_DIR="./arch-repo"

echo "Building Arch packages..."
mkdir -p "$REPO_DIR"

# Build packages
echo "Building plentysound..."
nix build .#aur --out-link result-aur
cp result-aur/*.pkg.tar.zst "$REPO_DIR/"

echo "Building plentysound-full..."
nix build .#aur-full --out-link result-aur-full
cp result-aur-full/*.pkg.tar.zst "$REPO_DIR/"

# Create repository database
cd "$REPO_DIR"
echo "Creating repository database..."

# repo-add needs to be run on Arch/Manjaro or in a container
if command -v repo-add &> /dev/null; then
    repo-add "$REPO_NAME.db.tar.gz" *.pkg.tar.zst
    echo "Repository created successfully!"
else
    echo "ERROR: repo-add not found. Running in Docker container..."
    docker run --rm -v "$(pwd):/repo" archlinux:latest bash -c "
        pacman -Sy --noconfirm pacman-contrib && \
        cd /repo && \
        repo-add $REPO_NAME.db.tar.gz *.pkg.tar.zst
    "
fi

echo ""
echo "Repository built in: $REPO_DIR"
echo "Files:"
ls -lh "$REPO_DIR"
echo ""
echo "Upload these files to GitHub releases or a web server."
echo ""
echo "Users can add your repository to /etc/pacman.conf:"
echo ""
echo "[plentysound]"
echo "SigLevel = Optional TrustAll"
echo "Server = https://github.com/yuri-potatoq/plentysound/releases/latest/download"
echo ""
echo "Then install with: sudo pacman -Sy plentysound"
