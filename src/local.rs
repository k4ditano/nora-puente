//! El socket local del puente (`$XDG_RUNTIME_DIR/nora-puente.sock`, un JSON
//! por línea): por aquí le hablan sus caras, como el extra de la barra de Bahía.
//!
//! Al conectarse, el cliente recibe el estado (`{"tipo":"estado",…}`) y otro
//! cada vez que cambia. Lo que manda el cliente:
//!   `{"tipo":"ajuste","volumen":0..100,"brillo":0..100}`  → a la consola
//!   `{"tipo":"aviso","titulo","texto","app"}`              → a la consola
//!   `{"tipo":"permiso","id",…}`  → a la consola; su `{"tipo":"respuesta","id",…}`
//!     vuelve solo a este cliente. Si el cliente se va antes, la consola recibe
//!     `{"tipo":"permiso_cancelado","id"}`; si se va la consola, el cliente recibe
//!     `{"tipo":"respuesta","id","sin_consola":true}`.
//!   `{"tipo":"app.instalar"|"app.borrar"|"app.lista"|"captura","pedido",…}` → a la
//!     consola; lo que contesta (`app.instalada`, `app.borrada`, `apps`, `captura`, con
//!     el mismo `pedido`) vuelve solo a este cliente (ver `apps.rs`).
//!   `{"tipo":"pulsar","botones":["A"],"ms":90}` → a la consola, como si se pulsaran
//!   `{"tipo":"escuchar","historia":bool}` → desde entonces recibe los sucesos de la
//!     consola: `log` (lo que escriben las apps con `nora.log`), `app.abierta`,
//!     `app.error`; con `historia`, antes los últimos que se guardan.
//!
//! `nora-puente seguir` es un cliente hecho para quien solo sabe lanzar un
//! programa y leer sus líneas (la lógica de un extra de Bahía): copia el socket a su
//! salida y su entrada al socket, y si el puente no está lo dice
//! (`{"tipo":"estado","puente":false}`) y vuelve a intentarlo.

use std::collections::{HashMap, VecDeque};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::estado::Estado;

pub fn ruta() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join("nora-puente.sock")
}

/// Lo que se le puede pedir a la consola desde el PC.
fn permitido(m: &Value) -> bool {
    matches!(m["tipo"].as_str(), Some("ajuste" | "aviso" | "pulsar"))
}

/// Los sucesos de la consola (log, app.abierta, app.error): los últimos, y quién los oye.
#[derive(Default)]
pub struct Sucesos {
    ultimos: VecDeque<String>,
    oyentes: Vec<Escritor>,
}
pub type Oyentes = Arc<Mutex<Sucesos>>;
const GUARDADOS: usize = 300;

/// Un suceso de la consola: se guarda y se manda a quien escuche.
pub fn difundir(oyentes: &Oyentes, m: &Value) {
    let mut m = m.clone();
    m["hora"] = Value::String(chrono::Local::now().format("%H:%M:%S").to_string());
    let linea = m.to_string();
    let mut s = oyentes.lock().unwrap();
    if s.ultimos.len() >= GUARDADOS {
        s.ultimos.pop_front();
    }
    s.ultimos.push_back(linea.clone());
    s.oyentes.retain(|w| escribe(w, &linea).is_ok());
}

/// Un cliente al que se le puede escribir desde varios hilos, línea a línea.
pub type Escritor = Arc<Mutex<UnixStream>>;
/// Los permisos que esperan respuesta de la consola: id → quién la espera.
pub type Esperando = Arc<Mutex<HashMap<String, Escritor>>>;

fn escribe(w: &Escritor, linea: &str) -> io::Result<()> {
    let mut l = linea.to_string();
    l.push('\n');
    w.lock().unwrap().write_all(l.as_bytes())
}

/// La consola ha contestado a un permiso (por su `id`) o a un pedido (por su `pedido`): a quien lo pidió.
pub fn responder(esperando: &Esperando, m: &Value) {
    let k = if m["pedido"].is_null() { &m["id"] } else { &m["pedido"] };
    let id = k.as_str().map(str::to_string).unwrap_or_else(|| k.to_string());
    if let Some(w) = esperando.lock().unwrap().remove(&id) {
        let _ = escribe(&w, &m.to_string());
    }
}

/// Se ha ido la consola: nadie va a contestar los permisos pendientes.
pub fn sin_consola(esperando: &Esperando) {
    for (id, w) in esperando.lock().unwrap().drain() {
        let _ = escribe(&w, &serde_json::json!({"tipo": "respuesta", "id": id, "pedido": id, "sin_consola": true}).to_string());
    }
}

fn cliente(s: UnixStream, estado: Arc<Estado>, a_consola: Arc<Mutex<Sender<String>>>, esperando: Esperando, oyentes: Oyentes) {
    let Ok(lee) = s.try_clone() else { return };
    let w: Escritor = Arc::new(Mutex::new(s));
    // lo que manda el cliente
    {
        let w = w.clone();
        thread::spawn(move || {
            let mut mios: Vec<String> = Vec::new();
            let mut pedidos: Vec<String> = Vec::new();
            for linea in BufReader::new(lee).lines() {
                let Ok(linea) = linea else { break };
                match serde_json::from_str::<Value>(&linea) {
                    Ok(m) if m["tipo"] == "permiso" => {
                        let Some(id) = m["id"].as_str() else { continue };
                        esperando.lock().unwrap().insert(id.to_string(), w.clone());
                        mios.push(id.to_string());
                        crate::log!("permiso: {} · {}", m["herramienta"].as_str().unwrap_or("?"),
                                    m["orden"].as_str().unwrap_or("").chars().take(80).collect::<String>());
                        let _ = a_consola.lock().unwrap().send(m.to_string());
                    }
                    Ok(m) if m["tipo"] == "escuchar" => {
                        let mut s = oyentes.lock().unwrap();
                        if m["historia"] == true {
                            for l in s.ultimos.clone() {
                                let _ = escribe(&w, &l);
                            }
                        }
                        s.oyentes.push(w.clone());
                    }
                    Ok(m) if matches!(m["tipo"].as_str(), Some("app.instalar" | "app.borrar" | "app.lista" | "app.abrir" | "app.borrar_datos" | "captura")) => {
                        let Some(p) = m["pedido"].as_str() else { continue };
                        esperando.lock().unwrap().insert(p.to_string(), w.clone());
                        if !pedidos.iter().any(|x| x == p) {
                            pedidos.push(p.to_string());
                        }
                        if m["tipo"] == "app.instalar" && m["fin"] == true {
                            crate::log!("app «{}» → consola", m["id"].as_str().unwrap_or("?"));
                        }
                        let _ = a_consola.lock().unwrap().send(m.to_string());
                    }
                    Ok(m) if permitido(&m) => {
                        let _ = a_consola.lock().unwrap().send(m.to_string());
                    }
                    Ok(m) => crate::log!("socket local: «{}» no se atiende", m["tipo"].as_str().unwrap_or("?")),
                    Err(_) => crate::log!("socket local: línea que no es JSON"),
                }
            }
            for p in pedidos {
                esperando.lock().unwrap().remove(&p);
            }
            // se fue sin esperar la respuesta (la terminal decidió, o se cansó): que la consola lo quite
            for id in mios {
                if esperando.lock().unwrap().remove(&id).is_some() {
                    let _ = a_consola.lock().unwrap().send(serde_json::json!({"tipo": "permiso_cancelado", "id": id}).to_string());
                }
            }
        });
    }
    // el estado, ahora y en cada cambio
    let (mut c, mut vista) = estado.ver();
    loop {
        if escribe(&w, &c.json().to_string()).is_err() {
            return;
        }
        loop {
            let (c2, v2) = estado.esperar(vista, Duration::from_secs(30));
            if v2 != vista {
                c = c2;
                vista = v2;
                break;
            }
        }
    }
}

/// Abre el socket local. `false` si ya hay otro puente corriendo (y entonces este sobra).
pub fn servir(estado: Arc<Estado>, a_consola: Sender<String>, esperando: Esperando, oyentes: Oyentes) -> bool {
    let ruta = ruta();
    if UnixStream::connect(&ruta).is_ok() {
        return false;
    }
    let _ = std::fs::remove_file(&ruta);
    let escucha = match UnixListener::bind(&ruta) {
        Ok(e) => e,
        Err(e) => {
            crate::log!("socket local: no se puede abrir {}: {e}", ruta.display());
            return true;
        }
    };
    let a_consola = Arc::new(Mutex::new(a_consola));
    thread::spawn(move || {
        for s in escucha.incoming().flatten() {
            let (e, a, p, o) = (estado.clone(), a_consola.clone(), esperando.clone(), oyentes.clone());
            thread::spawn(move || cliente(s, e, a, p, o));
        }
    });
    true
}

/// `nora-puente seguir`
pub fn seguir() -> ! {
    let ruta = ruta();
    // la entrada se lee una sola vez; cada conexión toma su parte
    let socket: Arc<Mutex<Option<UnixStream>>> = Arc::default();
    {
        let socket = socket.clone();
        thread::spawn(move || {
            for linea in io::stdin().lock().lines() {
                let Ok(linea) = linea else { break };
                // recién lanzado (o reconectando): se espera un poco al socket antes de tirar la línea
                for _ in 0..30 {
                    if let Some(s) = socket.lock().unwrap().as_mut() {
                        let _ = writeln!(s, "{linea}");
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
            // sin entrada (nadie nos escribe, o la cerró): se sigue informando igual; quien nos lanzó se
            // va cuando deja de leer, y entonces falla la escritura de abajo
        });
    }
    // quien nos lanzó (Bahía) se ha ido sin cerrarnos: nos ha adoptado init
    thread::spawn(|| {
        loop {
            if std::os::unix::process::parent_id() == 1 {
                std::process::exit(0);
            }
            thread::sleep(Duration::from_secs(2));
        }
    });
    let mut fuera = io::stdout();
    let mut dicho = false;
    loop {
        match UnixStream::connect(&ruta) {
            Ok(s) => {
                dicho = false;
                *socket.lock().unwrap() = s.try_clone().ok();
                for linea in BufReader::new(s).lines() {
                    let Ok(linea) = linea else { break };
                    if writeln!(fuera, "{linea}").and_then(|_| fuera.flush()).is_err() {
                        std::process::exit(0);
                    }
                }
                *socket.lock().unwrap() = None;
            }
            Err(_) => {}
        }
        if !dicho {
            dicho = true;
            if writeln!(fuera, r#"{{"tipo":"estado","puente":false,"conectada":false,"bar":{{"show":false}}}}"#)
                .and_then(|_| fuera.flush())
                .is_err()
            {
                std::process::exit(0);
            }
        }
        thread::sleep(Duration::from_secs(2));
    }
}
