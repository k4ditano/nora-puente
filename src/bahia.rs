//! El extra «Nora» para la barra de Bahía. Vive en este repo (`bahia/`) y va
//! dentro del binario; si el puente ve que usas Bahía (existe su carpeta de
//! configuración), lo deja en `~/.config/bahia/extras/`: Bahía dibuja la
//! píldora con lo que dice `nora-puente seguir` (su campo `bar`) y, al
//! pulsarla, abre la hoja `nora-hoja.plm` debajo. Sin Bahía no se toca nada.

use std::fs;
use std::path::PathBuf;

const ARCHIVOS: [(&str, &str); 4] = [
    ("nora.json", include_str!("../bahia/nora.json")),
    ("nora.svg", include_str!("../bahia/nora.svg")),
    ("nora-hoja.plm", include_str!("../bahia/nora-hoja.plm")),
    ("nora-hoja.luau", include_str!("../bahia/nora-hoja.luau")),
];

fn carpeta_bahia() -> Option<PathBuf> {
    // como Bahía: ${PLEAMAR_CONFIG:-$HOME/.config/bahia}
    let dir = std::env::var_os("PLEAMAR_CONFIG")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/bahia")))?;
    dir.is_dir().then_some(dir)
}

/// Pone (o pone al día) el extra si hay Bahía. Devuelve dónde, si lo ha escrito.
pub fn instalar() -> Result<Option<PathBuf>, String> {
    let Some(bahia) = carpeta_bahia() else { return Ok(None) };
    let yo = std::env::current_exe().map_err(|e| format!("¿dónde estoy?: {e}"))?;
    let yo = yo.to_string_lossy();
    // el widget de escritorio de la 0.1 (sustituido por el extra): fuera, si es nuestro
    for viejo in ["nora.plm", "nora.luau"] {
        let r = bahia.join("widgets").join(viejo);
        if fs::read_to_string(&r).is_ok_and(|t| t.contains("nora-puente")) {
            let _ = fs::remove_file(&r);
        }
    }
    let dir = bahia.join("extras");
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut escrito = false;
    for (nombre, plantilla) in ARCHIVOS {
        let ruta = dir.join(nombre);
        let texto = plantilla.replace("{{PUENTE}}", &yo);
        if fs::read_to_string(&ruta).ok().as_deref() != Some(texto.as_str()) {
            fs::write(&ruta, texto).map_err(|e| format!("{}: {e}", ruta.display()))?;
            escrito = true;
        }
    }
    Ok(escrito.then_some(dir.join("nora.json")))
}
