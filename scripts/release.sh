#!/bin/sh
# Publica una versión de Telar: sube la versión en Cargo.toml, hace commit, crea el tag
# vX.Y.Z y lo empuja. El workflow Release compila y publica el paquete en GitHub, y
# Telar lo ofrece como actualización.
#
# Uso: scripts/release.sh 0.3.0
set -eu
v=${1:?"Uso: scripts/release.sh X.Y.Z"}
case "$v" in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) echo "Versión inválida: $v (formato X.Y.Z)"; exit 1 ;;
esac
cd "$(dirname "$0")/.."
if [ -n "$(git status --porcelain)" ]; then
  echo "Hay cambios sin commitear; súbelos antes de publicar."; exit 1
fi
if git rev-parse -q --verify "refs/tags/v$v" >/dev/null; then
  echo "El tag v$v ya existe."; exit 1
fi
sed -i '' "s/^version = \".*\"/version = \"$v\"/" Cargo.toml
cargo build --release   # actualiza Cargo.lock
cargo test --release
git add Cargo.toml Cargo.lock
git commit -m "Versión $v"
git tag "v$v"
git push origin HEAD "v$v"
echo "Publicando v$v: https://github.com/cbustosm02/telar-terminal/actions"
