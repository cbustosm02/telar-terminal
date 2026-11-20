# Telar: si `TELAR_TAB_ID` está definido, corres dentro de la terminal Telar

Telar agrupa tabs, y cada tab suele tener una sesión de Claude Code. El comando `telar` (en `~/.local/bin`, o `"$TELAR_BIN"`) controla los otros tabs. `<tab>` puede ser el id, `self`, `grupo/n` o parte del título.

- `telar tabs`: JSON con los grupos y tabs, el estado de Claude en cada uno (`working`, `needs-you`, `done`, `idle`, `shell`), cuántos comandos de shell dejó corriendo (`claude_shells`) y su carpeta.
- `telar read <tab> [-n 200]`: texto de la pantalla y el scrollback de otro tab. Úsalo para ver logs de un servidor o de tests que corren en otro tab, en lugar de pedirle al usuario que copie la salida.
- `telar send <tab> "texto" [--enter]`: escribe en otro tab.
- `telar new [--group G] [--cwd DIR] [--focus] -- <comando>`: abre un tab, por defecto en tu mismo grupo y sin quitarle el foco al usuario. Imprime su id.
  - Ejemplo de agente en paralelo: `telar new -- claude "tarea"`.
- `telar wait <tab> [--state done] [--timeout 600]`: espera a que Claude en ese tab deje de trabajar.
- `telar wait <tab> --match 'Compiled successfully|Failed to compile' [--timeout 300]`: espera a que el tab muestre una línea nueva con alguno de esos textos y la imprime (lo que ya estaba en pantalla no cuenta). Úsalo tras editar código con un `ng serve` corriendo en otro tab, en lugar de leerlo una y otra vez con `read`; si dice que falló, lee el tab para ver el error.
- `telar suspend <tab> | --group G` / `telar resume …`: suspender cierra la shell y Claude y deja la última pantalla a la vista; reanudar retoma la sesión. Un tab suspendido no acepta `send`.
- `telar awake [on|off|auto]`: mantener el Mac despierto (caffeinate) siempre o mientras algún Claude trabaja.
- `telar group [--dir DIR] [--name N]`: muestra o cambia la carpeta y el nombre de tu grupo (los tabs nuevos del grupo abren en esa carpeta).

Reglas:
- Leer otros tabs (`tabs`, `read`, `status`) está bien cuando te sirve para la tarea.
- Abrir tabs o escribir en tabs que no creaste tú (`new`, `send`), suspender o reanudar tabs y cambiar `awake` solo cuando el usuario lo pida o lo haya autorizado para la tarea.
