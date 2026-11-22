//! Avisos para la consola: las notificaciones del escritorio y lo que se le
//! pida desde fuera (`nora-puente aviso …`, el hook `Stop` de Claude Code).
//! Cada aviso va a la consola como `{"tipo":"aviso","titulo","texto","app"}`
//! y allí cae desde arriba, encima de cualquier app.
//!
//! Las notificaciones se oyen en el bus de sesión con `busctl monitor`
//! (systemd), que da cada llamada a `Notify` como una línea JSON: no hace
//! falta ser el servidor de notificaciones ni quitarle el sitio a nadie.

use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

const REGLA: &str = "type='method_call',interface='org.freedesktop.Notifications',member='Notify'";

pub fn aviso(titulo: &str, texto: &str, app: &str) -> String {
    let corta = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    json!({"tipo": "aviso", "titulo": corta(titulo, 120), "texto": corta(texto, 300), "app": corta(app, 40)}).to_string()
}

/// Quita el marcado que admiten las notificaciones (`<b>`, `<a href…>`, `&amp;`…).
fn limpia(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut dentro = false;
    for c in s.chars() {
        match c {
            '<' => dentro = true,
            '>' if dentro => dentro = false,
            _ if !dentro => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Una línea de `busctl monitor --json=short` → el aviso, si merece la pena.
pub fn de_notificacion(linea: &str, ignoradas: &[String]) -> Option<String> {
    let m: Value = serde_json::from_str(linea).ok()?;
    if m["member"] != "Notify" {
        return None;
    }
    let d = m["payload"]["data"].as_array()?;
    // (app, reemplaza_a, icono, resumen, cuerpo, acciones, pistas, caduca)
    let app = d.first()?.as_str().unwrap_or("");
    if d.get(1).and_then(Value::as_u64).unwrap_or(0) != 0 {
        return None; // actualiza otra (una barra de progreso, un volumen): no es un aviso nuevo
    }
    let pistas = d.get(6).cloned().unwrap_or(Value::Null);
    if pistas["transient"]["data"] == true {
        return None;
    }
    if ignoradas.iter().any(|i| i.eq_ignore_ascii_case(app)) {
        return None;
    }
    let titulo = limpia(d.get(3)?.as_str().unwrap_or(""));
    let texto = limpia(d.get(4).and_then(Value::as_str).unwrap_or(""));
    if titulo.is_empty() && texto.is_empty() {
        return None;
    }
    let (titulo, texto) = if titulo.is_empty() { (texto, String::new()) } else { (titulo, texto) };
    Some(aviso(&titulo, &texto, app))
}

/// Servicio: oye las notificaciones y las manda a la consola.
/// `NORA_AVISOS_IGNORAR=app1,app2` deja fuera las de esas apps.
pub fn servicio_notificaciones(a_consola: Sender<String>) {
    let ignoradas: Vec<String> = std::env::var("NORA_AVISOS_IGNORAR")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let mut avisado = false;
    loop {
        let hijo = Command::new("busctl")
            .args(["--user", "monitor", "--json=short", "--match", REGLA])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        match hijo {
            Ok(mut h) => {
                if let Some(salida) = h.stdout.take() {
                    for linea in BufReader::new(salida).lines() {
                        let Ok(linea) = linea else { break };
                        if let Some(a) = de_notificacion(&linea, &ignoradas) {
                            if let Ok(v) = serde_json::from_str::<Value>(&a) {
                                crate::log!("aviso de «{}»: {}", v["app"].as_str().unwrap_or(""), v["titulo"].as_str().unwrap_or(""));
                            }
                            if a_consola.send(a).is_err() {
                                return;
                            }
                        }
                    }
                }
                let _ = h.wait();
            }
            Err(e) => {
                if !avisado {
                    crate::log!("avisos: no se puede oír el bus con busctl ({e}); sin notificaciones del escritorio");
                    avisado = true;
                }
            }
        }
        thread::sleep(Duration::from_secs(10));
    }
}

/// Manda una línea al puente por su socket local. Si el puente no está, no pasa nada.
fn al_puente(linea: &str) {
    if let Ok(mut s) = UnixStream::connect(crate::local::ruta()) {
        let _ = s.set_write_timeout(Some(Duration::from_secs(1)));
        let _ = writeln!(s, "{linea}");
    }
}

/// `nora-puente aviso <título> [texto]`
pub fn orden_aviso(args: &[String]) {
    let titulo = args.first().map(String::as_str).unwrap_or("Aviso");
    let texto = args.get(1..).map(|r| r.join(" ")).unwrap_or_default();
    al_puente(&aviso(titulo, &texto, "PC"));
}

/// El último mensaje de Claude en la conversación (lo último del registro JSONL).
fn ultimo_mensaje(registro: &str) -> Option<String> {
    let mut f = std::fs::File::open(registro).ok()?;
    let largo = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(largo.saturating_sub(512 * 1024))).ok()?;
    let mut cola = Vec::new();
    f.read_to_end(&mut cola).ok()?;
    let cola = String::from_utf8_lossy(&cola);
    for linea in cola.lines().rev() {
        let Ok(m) = serde_json::from_str::<Value>(linea) else { continue };
        if m["type"] != "assistant" {
            continue;
        }
        let texto: Vec<&str> = m["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| c["type"] == "text")
            .filter_map(|c| c["text"].as_str())
            .collect();
        let texto = texto.join(" ");
        let limpio = texto.split_whitespace().collect::<Vec<_>>().join(" ");
        if !limpio.is_empty() {
            return Some(limpio.replace(['*', '`', '#'], ""));
        }
    }
    None
}

/// `nora-puente claude-terminado`: para el hook `Stop` de Claude Code (lee su JSON por la entrada).
pub fn orden_claude_terminado() {
    let mut entrada = String::new();
    let _ = std::io::stdin().read_to_string(&mut entrada);
    let h: Value = serde_json::from_str(&entrada).unwrap_or(Value::Null);
    if h["stop_hook_active"] == true {
        return;
    }
    let proyecto = h["cwd"].as_str().and_then(|c| c.rsplit('/').next()).unwrap_or("").to_string();
    let texto = h["transcript_path"].as_str().and_then(ultimo_mensaje).unwrap_or_default();
    let app = if proyecto.is_empty() { "Claude Code".to_string() } else { format!("Claude · {proyecto}") };
    al_puente(&aviso("Claude ha terminado", &texto, &app));
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn notificacion_normal() {
        let l = r#"{"type":"method_call","member":"Notify","payload":{"type":"susssasa{sv}i","data":["Firefox",0,"","Descarga <b>lista</b>","informe.pdf &amp; más",[],{},5000]}}"#;
        let a: Value = serde_json::from_str(&de_notificacion(l, &[]).unwrap()).unwrap();
        assert_eq!(a["titulo"], "Descarga lista");
        assert_eq!(a["texto"], "informe.pdf & más");
        assert_eq!(a["app"], "Firefox");
    }

    #[test]
    fn fuera_las_que_no_son_avisos() {
        let reemplaza = r#"{"member":"Notify","payload":{"data":["vol",7,"","Volumen","50 %",[],{},1000]}}"#;
        let pasajera = r#"{"member":"Notify","payload":{"data":["x",0,"","Hola","",[],{"transient":{"type":"b","data":true}},1000]}}"#;
        let ignorada = r#"{"member":"Notify","payload":{"data":["Spotify",0,"","Canción","",[],{},1000]}}"#;
        assert!(de_notificacion(reemplaza, &[]).is_none());
        assert!(de_notificacion(pasajera, &[]).is_none());
        assert!(de_notificacion(ignorada, &["spotify".into()]).is_none());
    }
}
