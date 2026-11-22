//! Las fichas de la app Usos: cuánto queda de Claude Code y Codex.
//!
//! Los números salen de la misma herramienta que la barra de Bahía
//! (`~/Proyectos/bahia/tools/claude-usage`, o `NORA_CLAUDE_USAGE`), y se
//! mandan a la consola como el dato `usos.fichas`:
//! `[{titulo, sub, nota, color, barras: [{nombre, pct, detalle}], filas: [{nombre, valor}]}]`

use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Local, TimeZone};
use serde_json::{Value, json};

const DIAS: [&str; 7] = ["lun", "mar", "mié", "jue", "vie", "sáb", "dom"];

fn herramienta() -> String {
    std::env::var("NORA_CLAUDE_USAGE").unwrap_or_else(|_| {
        let casa = std::env::var("HOME").unwrap_or_default();
        format!("{casa}/Proyectos/bahia/tools/claude-usage")
    })
}

fn ahora() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs_f64()
}

fn hace(seg: f64) -> String {
    let seg = seg.max(0.0) as u64;
    match seg {
        0..90 => "ahora mismo".into(),
        90..3600 => format!("hace {} min", seg / 60),
        3600..86400 => format!("hace {} h", seg / 3600),
        _ => format!("hace {} días", seg / 86400),
    }
}

fn renueva(ts: Option<f64>, ahora: f64) -> String {
    let Some(ts) = ts else { return String::new() };
    let falta = ts - ahora;
    let Some(d) = Local.timestamp_opt(ts as i64, 0).single() else { return String::new() };
    let hora = d.format("%H:%M");
    if falta <= 0.0 {
        "ya se ha renovado".into()
    } else if falta < 3600.0 {
        format!("se renueva en {} min ({hora})", (falta / 60.0) as u64)
    } else if falta < 86400.0 {
        let f = falta as u64;
        format!("se renueva en {} h {} min ({hora})", f / 3600, f % 3600 / 60)
    } else {
        format!("se renueva el {} a las {hora}", DIAS[d.weekday().num_days_from_monday() as usize])
    }
}

fn color(id: &str) -> &'static str {
    match id {
        "claude" => "d97757",
        "codex" => "10a37f",
        _ => "8b90a5",
    }
}

fn nombre_ventana(n: &str) -> &str {
    match n {
        "5-hour" => "5 horas",
        "Weekly" => "Semanal",
        otro => otro,
    }
}

fn tokens(t: &Value) -> String {
    let tok = t["tok"].as_f64().unwrap_or(0.0);
    if tok >= 1e9 { format!("{:.1} mil M tokens", tok / 1e9) } else { format!("{:.0} M tokens", tok / 1e6) }
}

/// Convierte la salida de `claude-usage` en las fichas para la consola.
pub fn fichas_de(datos: &Value) -> Value {
    let ahora = ahora();
    let mut fichas = Vec::new();
    for a in datos["agents"].as_array().into_iter().flatten() {
        let id = a["id"].as_str().unwrap_or("");
        let mut sub = a["plan"].as_str().unwrap_or("").to_string();
        if let Some(m) = a["monthly"].as_f64() {
            sub += &format!(" · {m} $ al mes");
        }
        let origen = if a["origin"] == "server" { "del servidor" } else { "de sus registros locales" };
        let visto = a["observed"].as_f64().unwrap_or(ahora);
        let mut barras = Vec::new();
        for l in a["limits"].as_array().into_iter().flatten() {
            let usado = l["used"].as_f64();
            if l["minutes"].is_null() && usado.unwrap_or(0.0) == 0.0 {
                continue; // una ventana de modelo sin gastar
            }
            let (pct, detalle) = if l["pending"].as_bool().unwrap_or(false) {
                (-1, "se ha renovado; falta un dato nuevo".to_string())
            } else {
                (usado.map_or(-1, |u| u.round() as i64), renueva(l["renews"].as_f64(), ahora))
            };
            barras.push(json!({
                "nombre": nombre_ventana(l["name"].as_str().unwrap_or("")),
                "pct": pct,
                "detalle": detalle,
            }));
        }
        fichas.push(json!({
            "titulo": a["name"].as_str().unwrap_or(id),
            "sub": sub,
            "nota": format!("Dato {origen}, {}", hace(ahora - visto)),
            "color": color(id),
            "barras": barras,
            "filas": [],
        }));
    }
    // lo que habría costado Claude Code a precio de API
    let filas: Vec<Value> = [("Hoy", "today"), ("Esta semana", "week7"), ("Este mes", "month")]
        .iter()
        .map(|(nombre, clave)| {
            let t = &datos[*clave];
            json!({
                "nombre": format!("{nombre}  ·  {}", tokens(t)),
                "valor": format!("{:.0} $", t["cost"].as_f64().unwrap_or(0.0)),
            })
        })
        .collect();
    fichas.push(json!({
        "titulo": "A precio de API",
        "sub": "Lo que habría costado Claude Code",
        "nota": "Calculado de los registros de Claude Code",
        "color": "8b90a5",
        "barras": [],
        "filas": filas,
    }));
    Value::Array(fichas)
}

/// Ejecuta `claude-usage` y devuelve las fichas.
pub fn leer() -> Result<Value, String> {
    let ruta = herramienta();
    let salida = Command::new(&ruta).output().map_err(|e| format!("no se puede ejecutar {ruta}: {e}"))?;
    if !salida.status.success() {
        return Err(format!(
            "{ruta} ha fallado ({}): {}",
            salida.status,
            String::from_utf8_lossy(&salida.stderr).trim()
        ));
    }
    let datos: Value =
        serde_json::from_slice(&salida.stdout).map_err(|e| format!("{ruta} no ha dado JSON: {e}"))?;
    Ok(fichas_de(&datos))
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn fichas_basicas() {
        let datos = json!({
            "agents": [{
                "id": "claude", "name": "Claude Code", "plan": "Max 5×", "monthly": 100,
                "origin": "server", "observed": ahora() - 120.0,
                "limits": [
                    {"name": "5-hour", "minutes": 300, "used": 17.4, "renews": ahora() + 630.0, "pending": false},
                    {"name": "Fable", "minutes": null, "used": 0, "renews": null, "pending": false},
                    {"name": "Weekly", "minutes": 10080, "used": 3, "renews": null, "pending": true}
                ]
            }],
            "today": {"tok": 2.5e9, "cost": 40.2}, "week7": {"tok": 3e8, "cost": 99.6}
        });
        let f = fichas_de(&datos);
        let claude = &f[0];
        assert_eq!(claude["titulo"], "Claude Code");
        assert_eq!(claude["sub"], "Max 5× · 100 $ al mes");
        assert_eq!(claude["nota"], "Dato del servidor, hace 2 min");
        let barras = claude["barras"].as_array().unwrap();
        assert_eq!(barras.len(), 2, "la ventana de modelo sin gastar no sale");
        assert_eq!(barras[0]["nombre"], "5 horas");
        assert_eq!(barras[0]["pct"], 17);
        assert!(barras[0]["detalle"].as_str().unwrap().starts_with("se renueva en 10 min"));
        assert_eq!(barras[1]["pct"], -1);
        let api = &f[1];
        assert_eq!(api["filas"][0]["nombre"], "Hoy  ·  2.5 mil M tokens");
        assert_eq!(api["filas"][0]["valor"], "40 $");
        assert_eq!(api["filas"][2]["valor"], "0 $");
    }
}
