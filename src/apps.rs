//! Apps para la consola desde el PC: instalarlas, borrarlas, verlas, y
//! `vigilar` una carpeta para que cada vez que guardas aparezca en la consola
//! (recarga en caliente).
//!
//!     nora-puente instalar <carpeta> [--id X] [--abrir] [--limpiar]
//!     nora-puente vigilar  <carpeta> [--id X]
//!     nora-puente apps
//!     nora-puente borrar <id>
//!     nora-puente captura [archivo.png]
//!     nora-puente pulsar A ABAJO [--ms 90]
//!     nora-puente log [-f]
//!
//! Una app es una carpeta con `app.lua` (y si quiere `info.lua`, sonidos…).
//! Va por el socket del puente como `{"tipo":"app.instalar","pedido","id",
//! "archivos":{"ruta":base64},"fin":bool,"abrir":bool,"limpiar":bool}`, en
//! trozos de ~1 MB; la consola contesta `{"tipo":"app.instalada","pedido",…}`
//! al último y, si la app está abierta, la vuelve a abrir.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

/// Lo que cabe en un mensaje (antes del base64). La consola admite líneas de 4 MB.
const TROZO: usize = 1 << 20;
/// Un archivo más grande no va por aquí (para eso, `arranque/actualizar.sh`).
const MAX_ARCHIVO: u64 = 2 << 20;

pub fn base64(datos: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(datos.len().div_ceil(3) * 4);
    for c in datos.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

pub fn id_valido(id: &str) -> bool {
    !id.is_empty() && id != "sistema" && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Los archivos de la app: ruta relativa → (tamaño, cuándo cambió). Sin ocultos ni temporales.
fn archivos(raiz: &Path) -> BTreeMap<String, (u64, SystemTime)> {
    fn recorre(raiz: &Path, dir: &Path, out: &mut BTreeMap<String, (u64, SystemTime)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let nombre = e.file_name().to_string_lossy().to_string();
            if nombre.starts_with('.') || nombre.ends_with('~') || nombre.ends_with(".tmp") || nombre.ends_with(".swp") {
                continue;
            }
            let ruta = e.path();
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                recorre(raiz, &ruta, out);
            } else if md.is_file() {
                let rel = ruta.strip_prefix(raiz).unwrap().to_string_lossy().to_string();
                let valido = rel.split('/').all(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c)));
                if valido {
                    out.insert(rel, (md.len(), md.modified().unwrap_or(UNIX_EPOCH)));
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    recorre(raiz, raiz, &mut out);
    out
}

fn pedido_nuevo() -> String {
    let n = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("apps-{}-{}", std::process::id(), n % 1_000_000_000)
}

/// Una conversación con la consola por el socket del puente.
struct Puente {
    w: UnixStream,
    lector: BufReader<UnixStream>,
}

impl Puente {
    fn abrir() -> Result<Puente, String> {
        let s = UnixStream::connect(crate::local::ruta()).map_err(|_| "el puente no está (systemctl --user start nora-puente)".to_string())?;
        let w = s.try_clone().map_err(|e| e.to_string())?;
        let mut lector = BufReader::new(s);
        let _ = lector.get_ref().set_read_timeout(Some(Duration::from_secs(2)));
        let mut linea = String::new();
        lector.read_line(&mut linea).map_err(|e| e.to_string())?;
        let estado: Value = serde_json::from_str(&linea).unwrap_or(Value::Null);
        if estado["conectada"] != true {
            return Err("la consola no está conectada".into());
        }
        Ok(Puente { w, lector })
    }
    /// desde ahora, también los sucesos de la consola (log, app.abierta, app.error)
    fn escuchar(&mut self, historia: bool) -> Result<(), String> {
        self.manda(&json!({"tipo": "escuchar", "historia": historia}))
    }
    /// la siguiente línea que cumpla `vale`, o error al pasar el plazo
    fn espera_que(&mut self, plazo: Duration, vale: impl Fn(&Value) -> bool) -> Result<Value, String> {
        let hasta = Instant::now() + plazo;
        let mut linea = String::new();
        loop {
            let falta = hasta.saturating_duration_since(Instant::now());
            if falta.is_zero() {
                return Err("la consola no contesta".into());
            }
            let _ = self.lector.get_ref().set_read_timeout(Some(falta));
            linea.clear();
            match self.lector.read_line(&mut linea) {
                Ok(0) => return Err("el puente se ha ido".into()),
                Ok(_) => {}
                Err(_) => continue,
            }
            if let Ok(m) = serde_json::from_str::<Value>(&linea) {
                if vale(&m) {
                    return Ok(m);
                }
            }
        }
    }
    fn manda(&mut self, m: &Value) -> Result<(), String> {
        writeln!(self.w, "{m}").map_err(|e| format!("el puente se ha ido: {e}"))
    }
    /// espera la contestación a `pedido`
    fn espera(&mut self, pedido: &str, plazo: Duration) -> Result<Value, String> {
        let hasta = Instant::now() + plazo;
        let mut linea = String::new();
        loop {
            let falta = hasta.saturating_duration_since(Instant::now());
            if falta.is_zero() {
                return Err("la consola no contesta".into());
            }
            let _ = self.lector.get_ref().set_read_timeout(Some(falta));
            linea.clear();
            match self.lector.read_line(&mut linea) {
                Ok(0) => return Err("el puente se ha ido".into()),
                Ok(_) => {}
                Err(_) => continue,
            }
            let Ok(m) = serde_json::from_str::<Value>(&linea) else { continue };
            if m["pedido"] != pedido {
                continue;
            }
            if m["sin_consola"] == true {
                return Err("se ha ido la consola".into());
            }
            return Ok(m);
        }
    }
}

/// Instala `rutas` de la app (todas si es None). Devuelve la contestación de la consola.
fn instalar(raiz: &Path, id: &str, rutas: Option<&[String]>, abrir: bool, limpiar: bool) -> Result<Value, String> {
    instalar_clase(raiz, id, rutas, abrir, limpiar, false)
}

/// `tema`: la carpeta es un tema (tema.lua) y va a temas/<id>; `abrir` = ponerlo.
fn instalar_clase(raiz: &Path, id: &str, rutas: Option<&[String]>, abrir: bool, limpiar: bool, tema: bool) -> Result<Value, String> {
    let todas = archivos(raiz);
    let principal = if tema { "tema.lua" } else { "app.lua" };
    if !todas.contains_key(principal) {
        return Err(format!("{} no tiene {principal}", raiz.display()));
    }
    let lista: Vec<String> = match rutas {
        Some(r) => r.to_vec(),
        None => todas.keys().cloned().collect(),
    };
    let mut p = Puente::abrir()?;
    p.escuchar(false)?;
    let pedido = pedido_nuevo();
    let mut trozo = Map::new();
    let mut llevo = 0usize;
    let mut base = json!({"tipo": "app.instalar", "pedido": pedido, "id": id, "abrir": abrir, "limpiar": limpiar});
    if tema {
        base["clase"] = json!("tema");
    }
    let mut primero = true;
    for (i, r) in lista.iter().enumerate() {
        let ruta = raiz.join(r);
        let largo = std::fs::metadata(&ruta).map(|m| m.len()).unwrap_or(0);
        if largo > MAX_ARCHIVO {
            return Err(format!("{r} pesa {} KB; lo más grande que va así es {} KB", largo / 1024, MAX_ARCHIVO / 1024));
        }
        let datos = std::fs::read(&ruta).map_err(|e| format!("{r}: {e}"))?;
        llevo += datos.len();
        trozo.insert(r.clone(), Value::String(base64(&datos)));
        let ultimo = i + 1 == lista.len();
        if llevo >= TROZO || ultimo {
            let mut m = base.clone();
            m["archivos"] = Value::Object(std::mem::take(&mut trozo));
            m["fin"] = Value::Bool(ultimo);
            if !primero {
                m["limpiar"] = Value::Bool(false);
            }
            p.manda(&m)?;
            primero = false;
            llevo = 0;
        }
    }
    let mut r = p.espera(&pedido, Duration::from_secs(30))?;
    if r["ok"] != true {
        return Err(r["error"].as_str().unwrap_or("la consola no la ha instalado").to_string());
    }
    if r["recarga"] == true {
        espera_apertura(&mut p, id, &mut r);
    }
    Ok(r)
}

/// La consola va a abrir `id`: ¿carga bien o falla? (lo apunta en `r`)
fn espera_apertura(p: &mut Puente, id: &str, r: &mut Value) {
    {
        let abierta = p.espera_que(Duration::from_secs(5), |m| {
            m["id"] == id && (m["tipo"] == "app.abierta" || m["tipo"] == "app.error")
        });
        match abierta {
            Ok(m) if m["tipo"] == "app.error" => r["error_lua"] = m["error"].clone(),
            Ok(_) => r["cargada"] = Value::Bool(true),
            Err(_) => {}
        }
        // los primeros fotogramas también pueden fallar (app.entrar, app.pinta)
        if r["cargada"] == true {
            if let Ok(m) = p.espera_que(Duration::from_millis(600), |m| m["id"] == id && m["tipo"] == "app.error") {
                r["cargada"] = Value::Bool(false);
                r["error_lua"] = m["error"].clone();
            }
        }
    }
}

/// Cómo ha ido, en una línea (y el error de Lua entero, si lo hay).
fn resumen(r: &Value) -> (String, Option<String>) {
    let que = if r["nueva"] == true { "instalada" } else { "puesta al día" };
    let base = format!("{que} ({} archivos)", r["archivos"]);
    match (r["cargada"] == true, r["error_lua"].as_str()) {
        (_, Some(e)) => (format!("{base}, pero FALLA al abrirse"), Some(e.to_string())),
        (true, _) => (format!("{base}, abierta y funcionando"), None),
        _ => (base, None),
    }
}

struct Opciones {
    carpeta: PathBuf,
    id: String,
    abrir: bool,
    limpiar: bool,
}

fn opciones(args: &[String], orden: &str) -> Opciones {
    let mut carpeta = None;
    let mut id = None;
    let (mut abrir, mut limpiar) = (false, false);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--id" => {
                id = args.get(i + 1).cloned();
                i += 1;
            }
            "--abrir" => abrir = true,
            "--limpiar" => limpiar = true,
            "--poner" => abrir = true,
            otro if carpeta.is_none() => carpeta = Some(PathBuf::from(otro)),
            otro => sale(&format!("no sé qué es «{otro}»")),
        }
        i += 1;
    }
    let Some(carpeta) = carpeta else {
        sale(&format!("uso: nora-puente {orden} <carpeta de la app> [--id nombre]"))
    };
    let carpeta = std::fs::canonicalize(&carpeta).unwrap_or_else(|e| sale(&format!("{}: {e}", carpeta.display())));
    let id = id.unwrap_or_else(|| carpeta.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default());
    if !id_valido(&id) {
        sale(&format!("«{id}» no vale como nombre de app (letras, números, - y _); usa --id"));
    }
    Opciones { carpeta, id, abrir, limpiar }
}

fn sale(e: &str) -> ! {
    eprintln!("nora-puente: {e}");
    std::process::exit(1)
}

fn hora() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// `nora-puente instalar <carpeta>`
pub fn orden_instalar(args: &[String]) {
    let o = opciones(args, "instalar");
    match instalar(&o.carpeta, &o.id, None, o.abrir, o.limpiar) {
        Ok(r) => {
            let (linea, error) = resumen(&r);
            println!("«{}» {linea}", o.id);
            if let Some(e) = error {
                println!("{e}");
                std::process::exit(3);
            }
        }
        Err(e) => sale(&e),
    }
}

/// `nora-puente vigilar <carpeta>`: instala y, cada vez que cambia algo, manda lo cambiado.
pub fn orden_vigilar(args: &[String]) {
    let o = opciones(args, "vigilar");
    println!("vigilando {} → «{}» en la consola (Ctrl+C para dejarlo)", o.carpeta.display(), o.id);
    let mut visto = archivos(&o.carpeta);
    let mut falta_todo = true; // la primera vez, o tras no poder: la app entera
    let mut cambiados: Vec<String> = Vec::new();
    let mut quejado = String::new();
    let mut ultimo_intento = Instant::now() - Duration::from_secs(60);
    loop {
        let ahora = archivos(&o.carpeta);
        let nuevos: Vec<String> = ahora.iter().filter(|(r, m)| visto.get(*r) != Some(m)).map(|(r, _)| r.clone()).collect();
        for r in visto.keys().filter(|r| !ahora.contains_key(*r)) {
            println!("{} {r} ya no está (en la consola sigue; --limpiar para quitarlo)", hora());
        }
        if !nuevos.is_empty() {
            thread::sleep(Duration::from_millis(120)); // el editor puede estar aún escribiendo
            for r in nuevos {
                if !cambiados.contains(&r) {
                    cambiados.push(r);
                }
            }
            visto = archivos(&o.carpeta);
            ultimo_intento = Instant::now() - Duration::from_secs(60);
        } else {
            visto = ahora;
        }
        let toca = (falta_todo || !cambiados.is_empty()) && ultimo_intento.elapsed() >= Duration::from_secs(3);
        if toca {
            ultimo_intento = Instant::now();
            let rutas = if falta_todo { None } else { Some(cambiados.as_slice()) };
            match instalar(&o.carpeta, &o.id, rutas, falta_todo, false) {
                Ok(r) => {
                    let (linea, error) = resumen(&r);
                    if falta_todo {
                        println!("{} «{}» entera en la consola: {linea}", hora(), o.id);
                    } else {
                        println!("{} {} → consola: {linea}", hora(), cambiados.join(", "));
                    }
                    if let Some(e) = error {
                        println!("{e}");
                    }
                    falta_todo = false;
                    cambiados.clear();
                    quejado.clear();
                }
                Err(e) => {
                    if e != quejado {
                        println!("{} no se ha podido: {e} (se reintenta)", hora());
                        quejado = e.clone();
                    }
                    // si no hay consola, al volver se le manda entera (pudo reiniciarse)
                    if e.contains("consola") || e.contains("puente") {
                        falta_todo = true;
                        cambiados.clear();
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(250));
    }
}

/// `nora-puente apps`
pub fn orden_apps() {
    let r = Puente::abrir().and_then(|mut p| {
        let pedido = pedido_nuevo();
        p.manda(&json!({"tipo": "app.lista", "pedido": pedido}))?;
        p.espera(&pedido, Duration::from_secs(5))
    });
    match r {
        Ok(r) => {
            for a in r["apps"].as_array().into_iter().flatten() {
                println!("{:<16} {}", a["id"].as_str().unwrap_or("?"), a["nombre"].as_str().unwrap_or(""));
            }
        }
        Err(e) => sale(&e),
    }
}

/// `nora-puente borrar <id>`
pub fn orden_borrar(args: &[String]) {
    let Some(id) = args.first().filter(|i| id_valido(i)) else { sale("uso: nora-puente borrar <id de la app>") };
    let r = Puente::abrir().and_then(|mut p| {
        let pedido = pedido_nuevo();
        p.manda(&json!({"tipo": "app.borrar", "pedido": pedido, "id": id}))?;
        p.espera(&pedido, Duration::from_secs(10))
    });
    match r {
        Ok(r) if r["ok"] == true => println!("«{id}» borrada de la consola"),
        Ok(_) => sale(&format!("la consola no tiene «{id}» (o no se puede borrar)")),
        Err(e) => sale(&e),
    }
}

/// `nora-puente abrir <id>`
pub fn orden_abrir(args: &[String]) {
    let Some(id) = args.first().filter(|i| id_valido(i) || *i == "inicio") else { sale("uso: nora-puente abrir <id de la app>") };
    let mut p = Puente::abrir().unwrap_or_else(|e| sale(&e));
    p.escuchar(false).unwrap_or_else(|e| sale(&e));
    let pedido = pedido_nuevo();
    p.manda(&json!({"tipo": "app.abrir", "pedido": pedido, "id": id})).unwrap_or_else(|e| sale(&e));
    let mut r = p.espera(&pedido, Duration::from_secs(5)).unwrap_or_else(|e| sale(&e));
    if r["ok"] != true {
        sale(&format!("la consola no tiene «{id}»"));
    }
    espera_apertura(&mut p, id, &mut r);
    match (r["cargada"] == true, r["error_lua"].as_str()) {
        (_, Some(e)) => {
            println!("«{id}» FALLA al abrirse\n{e}");
            std::process::exit(3);
        }
        (true, _) => println!("«{id}» abierta y funcionando"),
        _ => println!("«{id}» pedida (sin noticias de si ha abierto)"),
    }
}

/// `nora-puente borrar-datos <id>`
pub fn orden_borrar_datos(args: &[String]) {
    let Some(id) = args.first().filter(|i| id_valido(i)) else { sale("uso: nora-puente borrar-datos <id de la app>") };
    let r = Puente::abrir().and_then(|mut p| {
        let pedido = pedido_nuevo();
        p.manda(&json!({"tipo": "app.borrar_datos", "pedido": pedido, "id": id}))?;
        p.espera(&pedido, Duration::from_secs(10))
    });
    match r {
        Ok(r) => println!("datos de «{id}» borrados ({} archivos)", r["archivos"]),
        Err(e) => sale(&e),
    }
}

/// `nora-puente tema <carpeta> [--poner] [--id X]`
pub fn orden_tema(args: &[String]) {
    let o = opciones(args, "tema");
    match instalar_clase(&o.carpeta, &o.id, None, o.abrir, o.limpiar, true) {
        Ok(r) => println!(
            "tema «{}» {} ({} archivos){}",
            o.id,
            if r["nueva"] == true { "instalado" } else { "puesto al día" },
            r["archivos"],
            if o.abrir { ", y puesto" } else { "" }
        ),
        Err(e) => sale(&e),
    }
}

/// `nora-puente temas`
pub fn orden_temas() {
    let r = Puente::abrir().and_then(|mut p| {
        let pedido = pedido_nuevo();
        p.manda(&json!({"tipo": "app.lista", "pedido": pedido, "clase": "tema"}))?;
        p.espera(&pedido, Duration::from_secs(5))
    });
    match r {
        Ok(r) => {
            for t in r["apps"].as_array().into_iter().flatten() {
                let id = t["id"].as_str().unwrap_or("?");
                let puesto = if r["tema"] == id { "  ← puesto" } else { "" };
                println!("{id:<16} {}{puesto}", t["nombre"].as_str().unwrap_or(""));
            }
        }
        Err(e) => sale(&e),
    }
}

/// `nora-puente poner-tema <id>`
pub fn orden_poner_tema(args: &[String]) {
    let Some(id) = args.first().filter(|i| id_valido(i)) else { sale("uso: nora-puente poner-tema <id del tema>") };
    let r = Puente::abrir().and_then(|mut p| {
        let pedido = pedido_nuevo();
        p.manda(&json!({"tipo": "tema.poner", "pedido": pedido, "id": id}))?;
        p.espera(&pedido, Duration::from_secs(5))
    });
    match r {
        Ok(r) if r["ok"] == true => println!("tema «{id}» puesto"),
        Ok(_) => sale(&format!("la consola no tiene el tema «{id}» (nora-puente temas)")),
        Err(e) => sale(&e),
    }
}

/// `nora-puente captura [archivo.png]`
pub fn orden_captura(args: &[String]) {
    let destino = PathBuf::from(args.first().map(String::as_str).unwrap_or("nora-captura.png"));
    let r = Puente::abrir().and_then(|mut p| {
        let pedido = pedido_nuevo();
        p.manda(&json!({"tipo": "captura", "pedido": pedido}))?;
        p.espera(&pedido, Duration::from_secs(10))
    });
    let r = r.unwrap_or_else(|e| sale(&e));
    let png = de_base64(r["png"].as_str().unwrap_or("")).unwrap_or_else(|| sale("la captura llega rota"));
    std::fs::write(&destino, &png).unwrap_or_else(|e| sale(&format!("{}: {e}", destino.display())));
    println!("{} ({}x{}, {} KB)", destino.display(), r["ancho"], r["alto"], png.len() / 1024);
}

fn de_base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut v, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let d = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => continue,
            _ => return None,
        } as u32;
        v = v << 6 | d;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((v >> bits) as u8);
        }
    }
    Some(out)
}

const BOTONES: [&str; 16] = [
    "A", "B", "X", "Y", "L", "R", "L2", "R2", "SELECT", "START", "MENU", "ARRIBA", "ABAJO", "IZQUIERDA", "DERECHA",
    "ENCENDIDO",
];

/// `nora-puente pulsar A ABAJO [--ms 90]`: uno tras otro; vuelve cuando han acabado.
pub fn orden_pulsar(args: &[String]) {
    let mut ms = 90u64;
    let mut botones = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--ms" {
            ms = args.get(i + 1).and_then(|m| m.parse().ok()).unwrap_or_else(|| sale("--ms necesita un número"));
            i += 1;
        } else {
            let b = args[i].to_uppercase();
            if !BOTONES.contains(&b.as_str()) {
                sale(&format!("«{}» no es un botón; valen: {}", args[i], BOTONES.join(" ")));
            }
            botones.push(b);
        }
        i += 1;
    }
    if botones.is_empty() {
        sale("uso: nora-puente pulsar A [ABAJO …] [--ms 90]");
    }
    let mut p = Puente::abrir().unwrap_or_else(|e| sale(&e));
    p.manda(&json!({"tipo": "pulsar", "botones": botones, "ms": ms})).unwrap_or_else(|e| sale(&e));
    thread::sleep(Duration::from_millis((ms + 70) * botones.len() as u64));
}

/// `nora-puente log [-f]`: lo último que han escrito las apps (y sus errores); con -f, sigue.
pub fn orden_log(args: &[String]) {
    let seguir = args.iter().any(|a| a == "-f" || a == "--seguir");
    let valor = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let ultimas: Option<usize> = valor("-n").map(|n| n.parse().unwrap_or_else(|_| sale("-n necesita un número")));
    let solo_app = valor("--app");
    let mut lineas: Vec<String> = Vec::new();
    let s = UnixStream::connect(crate::local::ruta()).unwrap_or_else(|_| sale("el puente no está"));
    let mut w = s.try_clone().unwrap_or_else(|e| sale(&e.to_string()));
    let _ = writeln!(w, "{}", json!({"tipo": "escuchar", "historia": true}));
    let lector = BufReader::new(s);
    // sin -f: lo que llegue enseguida (la historia) y se sale
    if !seguir {
        let _ = lector.get_ref().set_read_timeout(Some(Duration::from_millis(400)));
    }
    for linea in lector.lines() {
        let Ok(linea) = linea else { break };
        let Ok(m) = serde_json::from_str::<Value>(&linea) else { continue };
        let (h, id) = (m["hora"].as_str().unwrap_or(""), m["id"].as_str().unwrap_or("?"));
        if solo_app.as_deref().is_some_and(|a| a != id) {
            continue;
        }
        let texto = match m["tipo"].as_str().unwrap_or("") {
            "log" => format!("{h} {id}: {}", m["texto"].as_str().unwrap_or("")),
            "app.abierta" => format!("{h} {id}: abierta"),
            "app.error" => format!("{h} {id}: ERROR\n{}", m["error"].as_str().unwrap_or("")),
            _ => continue,
        };
        if seguir {
            println!("{texto}");
        } else {
            lineas.push(texto);
        }
    }
    let desde = ultimas.map(|n| lineas.len().saturating_sub(n)).unwrap_or(0);
    for l in &lineas[desde..] {
        println!("{l}");
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn base64_como_el_de_siempre() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"hola, Nora!"), "aG9sYSwgTm9yYSE=");
        for t in [&b""[..], b"a", b"ab", b"abc", b"\x00\xff\x10hola"] {
            assert_eq!(de_base64(&base64(t)).unwrap(), t);
        }
    }

    #[test]
    fn nombres_de_app() {
        assert!(id_valido("mi-juego_2"));
        assert!(!id_valido("sistema"));
        assert!(!id_valido("../x"));
        assert!(!id_valido(""));
    }
}
