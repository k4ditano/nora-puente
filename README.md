# nora-puente

**El servicio del PC que habla con [Nora OS](https://github.com/k4ditano/nora-os)**, el sistema propio de la Anbernic RG35XX original.

*The PC-side service for Nora OS: it feeds the handheld with data (AI usage, notifications, music, permission requests) and turns its buttons into actions on the desktop.*

> Estado: **0.1**. Se conecta a la consola, la pone en hora, le manda las fichas de uso de Claude y Codex y mantiene la conexión con latidos (si se corta, vuelve sola). Sustituye al prototipo `prototipo/usos-pc.py`.

## Uso

```sh
cargo install --path .
cp servicio/nora-puente.service ~/.config/systemd/user/
systemctl --user enable --now nora-puente       # arranca solo con la sesión
journalctl --user -u nora-puente -f             # qué hace

nora-puente --usos        # imprime las fichas de Usos y sale
nora-puente seguir        # cliente del socket local: estado por la salida, órdenes por la entrada
nora-puente bahia         # pone (o pone al día) el extra de la barra de Bahía
nora-puente --ip 10.0.0.5 # otra consola (o NORA_IP)
```

Necesita Nora 0.3 o más nueva (la que trae el servidor :7070). Los usos salen de `~/Proyectos/bahia/tools/claude-usage` (o `NORA_CLAUDE_USAGE`).

## Qué hará

- **Conectar con la consola** por el cable USB-C (red RNDIS, la consola en `192.168.7.2`) o por Wi-Fi, con latido y reconexión.
- **Darle datos**: usos de Claude Code y Codex, notificaciones del escritorio, lo que suena (MPRIS), estado del PC.
- **Recibir sus botones** y convertirlos en acciones: atajos, cambiar de escritorio, música, abrir una terminal.
- **Permisos de Claude Code con A/B**: un hook `PermissionRequest` pregunta al puente, la consola pita y enseña la orden, y el botón decide. Si la consola no está, el hook no espera y la terminal pregunta como siempre.
- **Instalar apps** (Lua) en la consola, y servir de API local para las shells del escritorio.

## En la barra de Bahía

Si usas [Bahía](https://github.com/k4ditano/bahia) (existe `~/.config/bahia`), el puente deja al arrancar el extra «Nora» en `~/.config/bahia/extras/`: una píldora en la barra con la batería de la consola que, al pulsarla, despliega una hoja con la app abierta, la conexión y el volumen y brillo con − y +. Sus fuentes están aquí, en `bahia/` (no en el repo de Bahía), y van dentro del binario; se reescriben si cambian, así que no se editan en `~/.config`. Sin Bahía no se toca nada. Necesita una Bahía con extras de otros programas (`extras/GUIDE.md` en su repo).

Bahía lanza `nora-puente seguir` y dibuja la píldora con su campo `bar`; la hoja (`nora-hoja.plm`) es una escena propia que también habla con el puente por `seguir`. Ese cliente usa el **socket local** (`$XDG_RUNTIME_DIR/nora-puente.sock`, un JSON por línea), que cualquier otra shell puede usar: recibe `{"tipo":"estado",…}` al conectarse y en cada cambio, y puede mandar `{"tipo":"ajuste","volumen":…,"brillo":…}`.

## Por qué va aparte

Para que Nora funcione con cualquier shell (o sin ninguna). Las barras del escritorio, como el extra «Nora» de Bahía, solo son una cara del puente: le piden el estado y le mandan órdenes por un socket local.

## Protocolo

TCP `:7070`, un JSON por línea. La consola hace de servidor (así no choca con el firewall del PC).

| Mensaje | Sentido | Qué hace |
|---|---|---|
| `{"tipo":"hola","version","bateria","cargando","app"}` | consola → PC | al conectar |
| `{"tipo":"latido"}` | los dos | cada 2 s; 6 s sin oír nada = desconectada |
| `{"tipo":"hora","epoch"}` | PC → consola | la pone en hora (reloj y RTC) |
| `{"tipo":"dato","app","nombre","valor"}` | PC → consola | la app lo lee con `nora.dato("app.nombre")` |
| `{"tipo":"ajuste","volumen","brillo"}` | los dos | 0–100; la consola contesta siempre con los valores que han quedado |
| `{"tipo":"estado","app","nombre_app","bateria","cargando"}` | consola → PC | cuando cambia la app abierta o la batería |

Ya implementado: lo de arriba. Pendiente: `aviso`, `permiso`/`respuesta`, `accion`, `app.instalar`, `captura`.

## Licencia

MIT.
