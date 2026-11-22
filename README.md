# nora-puente

**El servicio del PC que habla con [Nora OS](https://github.com/k4ditano/nora-os)**, el sistema propio de la Anbernic RG35XX original.

*The PC-side service for Nora OS: it feeds the handheld with data (AI usage, notifications, music, permission requests) and turns its buttons into actions on the desktop.*

> Estado: **plan**. Solo está el primer prototipo (`prototipo/usos-pc.py`), que manda a la consola las fichas de uso de Claude y Codex.

## Qué hará

- **Conectar con la consola** por el cable USB-C (red RNDIS, la consola en `192.168.7.2`) o por Wi-Fi, con latido y reconexión.
- **Darle datos**: usos de Claude Code y Codex, notificaciones del escritorio, lo que suena (MPRIS), estado del PC.
- **Recibir sus botones** y convertirlos en acciones: atajos, cambiar de escritorio, música, abrir una terminal.
- **Permisos de Claude Code con A/B**: un hook `PermissionRequest` pregunta al puente, la consola pita y enseña la orden, y el botón decide. Si la consola no está, el hook no espera y la terminal pregunta como siempre.
- **Instalar apps** (Lua) en la consola, y servir de API local para las shells del escritorio.

## Por qué va aparte

Para que Nora funcione con cualquier shell (o sin ninguna). Las barras del escritorio, como el extra «Nora» de Bahía, solo son una cara del puente: le piden el estado y le mandan órdenes por un socket local.

## Protocolo

TCP `:7070`, un JSON por línea. La consola hace de servidor (así no choca con el firewall del PC). Detalle en el diseño de Nora OS.

## Licencia

MIT.
