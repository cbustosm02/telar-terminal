#!/bin/bash
# Abre la versión en desarrollo como «Telar (vista previa).app», con su propia sesión
# (~/.config/telar/vista-previa.json) y su propio identificador: no toca Telar.app ni la
# sesión real, y las notificaciones nativas se pueden probar (necesitan un paquete).
#
# Uso: scripts/vista-previa.sh [VAR=valor …]   (variables extra para la prueba)
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release
APP="target/vista-previa/Telar (vista previa).app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/telar "$APP/Contents/MacOS/telar"
if [ -f /Applications/Telar.app/Contents/Resources/Telar.icns ]; then
  cp /Applications/Telar.app/Contents/Resources/Telar.icns "$APP/Contents/Resources/"
fi
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Telar (vista previa)</string>
  <key>CFBundleDisplayName</key><string>Telar (vista previa)</string>
  <key>CFBundleIdentifier</key><string>dev.cbustos.telar.vista-previa</string>
  <key>CFBundleExecutable</key><string>telar</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleIconFile</key><string>Telar</string>
  <key>CFBundleShortVersionString</key><string>0.0.0</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
# Con la firma local estable (la misma de Telar.app; se crea si falta), los permisos de
# macOS de la vista previa sobreviven a cada recompilación.
"$APP/Contents/MacOS/telar" __firmar "$APP" || echo "Aviso: la vista previa quedó firmada ad hoc"
# Cerrarla como con ⌘Q (guarda su sesión y dónde estaba la ventana); si no responde, a la fuerza.
if pgrep -f "target/vista-previa/Telar" >/dev/null; then
  osascript -e 'tell application "Telar (vista previa)" to quit' >/dev/null 2>&1 || true
  for _ in 1 2 3 4 5 6 7 8 9 10; do pgrep -f "target/vista-previa/Telar" >/dev/null || break; sleep 0.3; done
fi
pkill -f "target/vista-previa/Telar" 2>/dev/null || true
pkill -f "target/release/telar$" 2>/dev/null || true
# Con `open` (no como hijo de esta shell), macOS la trata como app propia: los permisos
# que pida son de la vista previa, no de la terminal desde la que se lanzó.
ENVS=(--env "TELAR_SESSION_FILE=$HOME/.config/telar/vista-previa.json")
for v in "$@"; do ENVS+=(--env "$v"); done
open -n "$APP" "${ENVS[@]}"
echo "Vista previa abierta"
