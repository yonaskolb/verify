#!/bin/sh
set -e

REPO="yonaskolb/verify"
BINARY="verify"

# Detect platform
OS=$(uname -s)
ARCH=$(uname -m)

case "$OS-$ARCH" in
  Linux-x86_64)   TARGET="x86_64-unknown-linux-gnu" ;;
  Darwin-x86_64)  TARGET="x86_64-apple-darwin" ;;
  Darwin-arm64)   TARGET="aarch64-apple-darwin" ;;
  *) echo "Unsupported platform: $OS-$ARCH"; exit 1 ;;
esac

URL="https://github.com/$REPO/releases/latest/download/$BINARY-$TARGET.tar.gz"

echo "Downloading $BINARY for $TARGET..."
curl -fsSL "$URL" | tar xz

echo "Installing to /usr/local/bin (may require sudo)..."
sudo mv "$BINARY" /usr/local/bin/

echo "Done! Run '$BINARY --help' to get started."
