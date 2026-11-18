# Telar: si `TELAR_TAB_ID` está definido, corres dentro de la terminal Telar

Telar agrupa tabs, y cada tab suele tener una sesión de Claude Code. El comando `telar` (en `~/.local/bin`, o `"$TELAR_BIN"`) controla los otros tabs. `<tab>` puede ser el id, `self`, `grupo/n` o parte del título.

- `telar tabs`: JSON con los grupos y tabs, el estado de Claude en cada uno (`working`, `needs-you`, `done`, `idle`, `shell`) y su carpeta.
- `telar read <tab> [-n 200]`: texto de la pantalla y el scrollback de otro tab. Úsalo para ver logs de un servidor o de tests que corren en otro tab, en lugar de pedirle al usuario que copie la salida.
- `telar send <tab> "texto" [--enter]`: escribe en otro tab.
- `telar new [--group G] [--cwd DIR] [--focus] -- <comando>`: abre un tab, por defecto en tu mismo grupo y sin quitarle el foco al usuario. Imprime su id.
  - Ejemplo de agente en paralelo: `telar new -- claude "tarea"`.
- `telar wait <tab> [--state done] [--timeout 600]`: espera a que Claude en ese tab deje de trabajar.
- `telar group [--dir DIR] [--name N]`: muestra o cambia la carpeta y el nombre de tu grupo (los tabs nuevos del grupo abren en esa carpeta).

Reglas:
- Leer otros tabs (`tabs`, `read`, `status`) está bien cuando te sirve para la tarea.
- Abrir tabs o escribir en tabs que no creaste tú (`new`, `send`) solo cuando el usuario lo pida o lo haya autorizado para la tarea.
