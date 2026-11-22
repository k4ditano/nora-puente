#!/usr/bin/env python3
"""Manda a la RG35XX las fichas de uso de Claude y Codex.

Saca los numeros de la misma herramienta que la barra de Bahia
(~/Proyectos/bahia/tools/claude-usage) y los deja en /tmp/usos.txt de la
consola, por telnet sobre el cable USB.

    python3 usos-pc.py            # cada minuto, para siempre
    python3 usos-pc.py --una      # una vez y sale
    python3 usos-pc.py --ver      # solo imprime lo que mandaria
"""
import json, subprocess, sys, time, os, socket
from datetime import datetime

# consola.py vive en el repo nora-os (herramientas/)
sys.path.insert(0, os.path.expanduser(os.environ.get("NORA_HERRAMIENTAS", "~/Proyectos/nora-os/herramientas")))
import consola

IP = "192.168.7.2"
USAGE = os.path.expanduser("~/Proyectos/bahia/tools/claude-usage")
COLOR = {"claude": "d97757", "codex": "10a37f"}
DIAS = ["lun", "mar", "mié", "jue", "vie", "sáb", "dom"]
NOMBRE = {"5-hour": "5 horas", "Weekly": "Semanal"}


def hace(seg):
    seg = max(0, int(seg))
    if seg < 90: return "ahora mismo"
    if seg < 3600: return f"hace {seg // 60} min"
    if seg < 86400: return f"hace {seg // 3600} h"
    return f"hace {seg // 86400} días"


def renueva(ts, ahora):
    if not ts: return ""
    falta = ts - ahora
    d = datetime.fromtimestamp(ts)
    hora = d.strftime("%H:%M")
    if falta <= 0: return "ya se ha renovado"
    if falta < 3600: return f"se renueva en {int(falta // 60)} min ({hora})"
    if falta < 86400: return f"se renueva en {int(falta // 3600)} h {int(falta % 3600 // 60)} min ({hora})"
    return f"se renueva el {DIAS[d.weekday()]} a las {hora}"


def fichas():
    datos = json.loads(subprocess.run([USAGE], capture_output=True, text=True, timeout=60).stdout)
    ahora = time.time()
    out = []
    for a in datos.get("agents", []):
        plan = a.get("plan") or ""
        sub = plan + (f" · {a['monthly']} $ al mes" if a.get("monthly") else "")
        origen = "del servidor" if a.get("origin") == "server" else "de sus registros locales"
        nota = f"Dato {origen}, {hace(ahora - (a.get('observed') or ahora))}"
        out.append(["F", a.get("name", a["id"]), sub, nota, COLOR.get(a["id"], "8b90a5")])
        for l in a.get("limits", []):
            if l.get("minutes") is None and not l.get("used"):
                continue                                  # una ventana de modelo sin gastar
            nombre = NOMBRE.get(l["name"], l["name"])
            pct = l.get("used")
            if l.get("pending"):
                pct, det = -1, "se ha renovado; falta un dato nuevo"
            else:
                det = renueva(l.get("renews"), ahora)
            out.append(["B", nombre, -1 if pct is None else round(pct), det])
    # resumen de lo que habria costado Claude a precio de API
    hoy, sem, mes = datos.get("today", {}), datos.get("week7", {}), datos.get("month", {})
    tok = lambda t: (f"{t.get('tok', 0) / 1e9:.1f} mil M tokens" if t.get('tok', 0) >= 1e9 else f"{t.get('tok', 0) / 1e6:.0f} M tokens")
    out.append(["F", "A precio de API", "Lo que habría costado Claude Code", "Calculado de los registros de Claude Code", "8b90a5"])
    for nom, t in (("Hoy", hoy), ("Esta semana", sem), ("Este mes", mes)):
        out.append(["T", f"{nom}  ·  {tok(t)}", f"{t.get('cost', 0):.0f} $"])
    return "\n".join("\t".join(str(c).replace("\t", " ") for c in f) for f in out) + "\n"


def mandar(texto):
    """Por nc y no por la linea de ordenes: esa se come tabuladores y tildes."""
    datos = texto.encode("latin-1", "replace")      # la consola pinta Latin-1
    consola.run(IP, "(sh -c 'busybox nc -l -p 9001 > /tmp/usos.new && mv /tmp/usos.new /tmp/usos.txt' &)")
    time.sleep(0.5)
    s = socket.create_connection((IP, 9001), timeout=5)
    s.sendall(datos)
    s.shutdown(socket.SHUT_WR)
    time.sleep(0.3)
    s.close()


if __name__ == "__main__":
    if "--ver" in sys.argv:
        print(fichas()); sys.exit()
    while True:
        try:
            mandar(fichas())
            print(time.strftime("%H:%M:%S"), "mandado", flush=True)
        except Exception as e:
            print(time.strftime("%H:%M:%S"), "fallo:", e, flush=True)
        if "--una" in sys.argv: break
        time.sleep(60)
