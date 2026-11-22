//! `nora-puente permiso`: el hook `PermissionRequest` de Claude Code.
//!
//! Lee lo que Claude quiere hacer (su JSON por la entrada), se lo enseña a la
//! consola por el puente y espera el botón: A permite, B rechaza, Y permite y
//! añade la regla que sugiere Claude Code («no volver a preguntar»). Si la
//! consola no está conectada, o no se contesta en `PLAZO`, no dice nada: Claude
//! Code pregunta en la terminal como siempre.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// Lo que se espera a la consola antes de dejar que decida la terminal.
const PLAZO: u64 = 60;

fn corta(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out
}

/// (orden, detalle) para enseñar en la consola, según la herramienta.
fn que_hace(herramienta: &str, e: &Value) -> (String, String) {
    let txt = |k: &str| e[k].as_str().unwrap_or("").to_string();
    match herramienta {
        "Bash" => (txt("command"), txt("description")),
        "Edit" | "MultiEdit" => {
            let viejo = corta(&txt("old_string").split_whitespace().collect::<Vec<_>>().join(" "), 60);
            let nuevo = corta(&txt("new_string").split_whitespace().collect::<Vec<_>>().join(" "), 60);
            (txt("file_path"), if viejo.is_empty() { String::new() } else { format!("«{viejo}» → «{nuevo}»") })
        }
        "Write" => {
            let lineas = txt("content").lines().count();
            (txt("file_path"), format!("escribe el archivo entero ({lineas} líneas)"))
        }
        "NotebookEdit" => (txt("notebook_path"), String::new()),
        "WebFetch" => (txt("url"), corta(&txt("prompt"), 120)),
        "WebSearch" => (txt("query"), String::new()),
        "Read" | "Glob" | "Grep" => {
            let que = [txt("file_path"), txt("pattern"), txt("path")].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>();
            (que.join("  ·  "), String::new())
        }
        _ => {
            // otra herramienta (una de MCP…): sus argumentos, en una línea
            let mut s = e.to_string();
            if s == "null" || s == "{}" {
                s = String::new();
            }
            (corta(&s, 600), String::new())
        }
    }
}

fn decision(d: Value) {
    let salida = json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": d}});
    println!("{salida}");
}

pub fn orden_permiso() {
    let mut entrada = String::new();
    let _ = std::io::stdin().read_to_string(&mut entrada);
    let Ok(h) = serde_json::from_str::<Value>(&entrada) else { return };
    let Ok(s) = UnixStream::connect(crate::local::ruta()) else { return }; // sin puente: la terminal
    let Ok(mut w) = s.try_clone() else { return };
    let mut lector = BufReader::new(s);
    let _ = lector.get_ref().set_read_timeout(Some(Duration::from_secs(2)));

    // lo primero que dice el puente es su estado: sin consola no se le pregunta
    let mut linea = String::new();
    if lector.read_line(&mut linea).unwrap_or(0) == 0 {
        return;
    }
    let estado: Value = serde_json::from_str(&linea).unwrap_or(Value::Null);
    if estado["conectada"] != true {
        return;
    }

    let herramienta = h["tool_name"].as_str().unwrap_or("Herramienta").to_string();
    let (orden, detalle) = que_hace(&herramienta, &h["tool_input"]);
    let sugerencias = h["permission_suggestions"].as_array().cloned().unwrap_or_default();
    let carpeta = h["cwd"].as_str().and_then(|c| c.rsplit('/').next()).unwrap_or("").to_string();
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let id = format!("{}-{}", std::process::id(), nanos % 1_000_000_000);
    let pide = json!({
        "tipo": "permiso", "id": id, "herramienta": herramienta,
        "orden": corta(&orden, 2000), "detalle": corta(&detalle, 200), "carpeta": carpeta,
        "siempre": !sugerencias.is_empty(), "plazo": PLAZO,
    });
    if writeln!(w, "{pide}").is_err() {
        return;
    }

    let hasta = Instant::now() + Duration::from_secs(PLAZO);
    loop {
        let falta = hasta.saturating_duration_since(Instant::now());
        if falta.is_zero() {
            return; // sin respuesta: decide la terminal (el puente avisa a la consola al cerrarse esto)
        }
        let _ = lector.get_ref().set_read_timeout(Some(falta));
        linea.clear();
        match lector.read_line(&mut linea) {
            Ok(0) => return,
            Ok(_) => {}
            Err(_) => return,
        }
        let Ok(m) = serde_json::from_str::<Value>(&linea) else { continue };
        if m["tipo"] != "respuesta" || m["id"] != id.as_str() {
            continue; // el estado, que sigue llegando
        }
        if m["sin_consola"] == true {
            return;
        }
        if m["ok"] == true {
            let mut d = json!({"behavior": "allow"});
            if m["siempre"] == true && !sugerencias.is_empty() {
                d["updatedPermissions"] = Value::Array(sugerencias);
            }
            decision(d);
        } else {
            decision(json!({"behavior": "deny", "message": "Rechazado desde la consola (Nora)."}));
        }
        return;
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn que_hace_cada_herramienta() {
        let (o, d) = que_hace("Bash", &json!({"command": "git push origin main", "description": "Sube la rama"}));
        assert_eq!((o.as_str(), d.as_str()), ("git push origin main", "Sube la rama"));
        let (o, d) = que_hace("Edit", &json!({"file_path": "/a/b.rs", "old_string": "x  =\n 1", "new_string": "x = 2"}));
        assert_eq!(o, "/a/b.rs");
        assert_eq!(d, "«x = 1» → «x = 2»");
        let (o, _) = que_hace("mcp__edinot__write_note", &json!({"name": "Nota"}));
        assert_eq!(o, r#"{"name":"Nota"}"#);
    }
}
