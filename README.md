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
nora-puente aviso "Título" "texto"   # un aviso en la consola
nora-puente --ip 10.0.0.5 # otra consola (o NORA_IP)

nora-puente instalar mi-juego/          # pone la app en la consola (--abrir, --limpiar, --id nombre)
nora-puente vigilar mi-juego/           # y cada vez que guardas, la recarga en caliente
nora-puente apps                        # las apps que tiene la consola
nora-puente borrar mi-juego             # quita una
nora-puente captura pantalla.png        # la pantalla de la consola, en PNG
nora-puente pulsar A ABAJO --ms 90      # pulsar botones de la consola desde el PC
nora-puente log [-f]                    # lo que escriben las apps (nora.log) y sus errores
nora-puente tema mi-tema/ --poner       # instala un tema (y lo pone); temas, poner-tema <id>
nora-puente actualizar                  # pone en la consola lo que ha cambiado del repo de Nora OS y la relanza
```

`instalar` dice si la app ha abierto bien o el error de Lua con su traza (y sale con
código 3): con `captura` y `pulsar` un agente puede hacer, mirar y jugar una app solo.
La skill para eso está en el repo de Nora OS (`skill/`).

Necesita Nora 0.3 o más nueva (la que trae el servidor :7070); para las apps, Nora 0.4; para `captura`, `pulsar`, `log` y los errores al instalar, Nora 0.5. Los usos salen de `~/Proyectos/bahia/tools/claude-usage` (o `NORA_CLAUDE_USAGE`).

## Qué hará

- **Conectar con la consola** por el cable USB-C (red RNDIS, la consola en `192.168.7.2`) o por Wi-Fi, con latido y reconexión.
- **Darle datos**: usos de Claude Code y Codex, notificaciones del escritorio, lo que suena (MPRIS), estado del PC.
- **Recibir sus botones** y convertirlos en acciones: atajos, cambiar de escritorio, música, abrir una terminal.
- **Permisos de Claude Code con A/B**: un hook `PermissionRequest` pregunta al puente, la consola pita y enseña la orden, y el botón decide. Si la consola no está, el hook no espera y la terminal pregunta como siempre.
- ~~**Instalar apps** (Lua) en la consola~~ (hecho: `instalar` y `vigilar`), y servir de API local para las shells del escritorio.

## En la barra de Bahía

Si usas Bahía (existe `~/.config/bahia`), el puente deja al arrancar el extra «Nora» en `~/.config/bahia/extras/`: una píldora en la barra con la batería de la consola que, al pulsarla, despliega una hoja con la app abierta, la conexión y el volumen y brillo con − y +. Sus fuentes están aquí, en `bahia/` (no en el repo de Bahía), y van dentro del binario; se reescriben si cambian, así que no se editan en `~/.config`. Sin Bahía no se toca nada. Necesita una Bahía con extras de otros programas (`extras/GUIDE.md` en su repo).

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
| `{"tipo":"aviso","titulo","texto","app","accion"}` | PC → consola | cae desde arriba; con A la consola manda `{"tipo":"accion","nombre"}` |
| `{"tipo":"permiso","id","herramienta","orden","detalle","carpeta","siempre","plazo"}` | PC → consola | pantalla de permiso: A permite, B rechaza, Y siempre |
| `{"tipo":"respuesta","id","ok","siempre"}` | consola → PC | la respuesta, que vuelve al hook que preguntó |
| `{"tipo":"permiso_cancelado","id"}` | PC → consola | la terminal ya decidió: fuera la pantalla |
| `{"tipo":"app.instalar","pedido","id","archivos":{"ruta":base64},"fin","abrir","limpiar"}` | PC → consola | escribe los archivos en `apps/<id>/` (en trozos de ~1 MB, mismo `pedido`); con `fin` contesta y, si la app está abierta (o `abrir`), la vuelve a abrir |
| `{"tipo":"app.instalada","pedido","id","ok","nueva","archivos","error"}` | consola → PC | la contestación a `app.instalar` |
| `{"tipo":"app.borrar","pedido","id"}` → `{"tipo":"app.borrada","pedido","id","ok"}` | PC ⇄ consola | quita una app |
| `{"tipo":"app.lista","pedido"}` → `{"tipo":"apps","pedido","apps":[{"id","nombre"}]}` | PC ⇄ consola | las apps instaladas |
| `{"tipo":"captura","pedido"}` → `{"tipo":"captura","pedido","ancho","alto","png"}` | PC ⇄ consola | la pantalla en PNG (base64) |
| `{"tipo":"pulsar","botones":["A"],"ms"}` | PC → consola | como si se pulsaran, uno tras otro |
| `{"tipo":"log","id","texto"}` | consola → PC | lo que escribe una app con `nora.log` |
| `{"tipo":"app.abierta","id"}` / `{"tipo":"app.error","id","error"}` | consola → PC | una app se ha abierto / ha fallado (con la traza) |

Ya implementado: lo de arriba (la `accion` de un aviso llega al puente, pero aún no hace nada). Pendiente: `app.instalar`, `captura`.

## Claude Code

Dos hooks en `~/.claude/settings.json`:

```json
"Stop":              [{ "hooks": [{ "type": "command", "command": "~/.cargo/bin/nora-puente claude-terminado", "async": true }] }],
"PermissionRequest": [{ "hooks": [{ "type": "command", "command": "~/.cargo/bin/nora-puente permiso", "timeout": 75 }] }]
```

- **`claude-terminado`**: un aviso «Claude ha terminado» con el proyecto y el principio de la respuesta.
- **`permiso`**: la consola pita y enseña lo que Claude quiere hacer; A permite, B rechaza, Y permite y añade la regla que sugiere Claude Code. Si la consola no está conectada, o no se contesta en 60 s, no dice nada y pregunta la terminal como siempre.

## Licencia

MIT.
