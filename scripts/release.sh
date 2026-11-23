#!/bin/sh
# Publica una versión de Telar desde este Mac (Apple Silicon): sube la versión en
# Cargo.toml, corre las pruebas, compila, arma el paquete con su SHA-256, sube el commit y
# crea el release vX.Y.Z en GitHub con el paquete (gh crea también el tag). Telar lo
# ofrece como actualización.
#
# El workflow Release queda de respaldo: si el release ya tiene su paquete no hace nada;
# si no (falló la subida desde aquí), lo compila y lo sube él.
#
# Uso: scripts/release.sh 0.16.0
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
if git rev-parse -q --verify "refs/tags/v$v" >/dev/null || git ls-remote --exit-code --tags origin "v$v" >/dev/null 2>&1; then
  echo "El tag v$v ya existe."; exit 1
fi
if [ "$(uname -m)" != "arm64" ]; then
  echo "El paquete es para Apple Silicon: publica desde un Mac con Apple Silicon."; exit 1
fi
# La cuenta personal de gh, la dueña del repo (si no, la que esté activa).
export GH_TOKEN="${GH_TOKEN:-$(gh auth token --user cbustosm02 2>/dev/null || gh auth token)}"

sed -i '' "s/^version = \".*\"/version = \"$v\"/" Cargo.toml
cargo build --release   # actualiza Cargo.lock
cargo test --release

# Paquete: el binario firmado ad hoc (el actualizador vuelve a firmar el .app con la firma
# local estable al instalarlo).
dist=target/dist
rm -rf "$dist"
mkdir -p "$dist"
cp target/release/telar "$dist/telar"
codesign --force --sign - "$dist/telar"
lipo -archs "$dist/telar" | grep -qx arm64
"$dist/telar" --version | grep -q "$v"
pkg="telar-$v-macos-arm64.tar.gz"
tar -czf "$dist/$pkg" -C "$dist" telar
(cd "$dist" && shasum -a 256 "$pkg" > "$pkg.sha256")

git add Cargo.toml Cargo.lock
git commit -m "Versión $v"
git push origin HEAD
sha=$(git rev-parse HEAD)
# El release crea el tag en GitHub sobre este commit, con el paquete ya adentro.
if gh release create "v$v" "$dist/$pkg" "$dist/$pkg.sha256" --target "$sha" --title "Telar $v" --generate-notes; then
  git tag "v$v" "$sha"
  echo "Publicado v$v: https://github.com/cbustosm02/telar-terminal/releases/tag/v$v"
else
  # Sin poder publicar desde aquí: el tag dispara el workflow, que compila y publica.
  echo "No se pudo crear el release desde este Mac; lo hará el workflow Release."
  git tag "v$v" "$sha"
  # Si gh alcanzó a crear el release (y el tag) sin el paquete, el tag ya está allá.
  git push origin "v$v" 2>/dev/null || true
  echo "Si no arranca solo, lánzalo a mano: gh workflow run Release -f tag=v$v"
  echo "Siguiendo en: https://github.com/cbustosm02/telar-terminal/actions"
fi
