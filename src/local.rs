//! El socket local del puente (`$XDG_RUNTIME_DIR/nora-puente.sock`, un JSON
//! por línea): por aquí le hablan sus caras, como el extra de la barra de Bahía.
//!
//! Al conectarse, el cliente recibe el estado (`{"tipo":"estado",…}`) y otro
//! cada vez que cambia. Lo que manda el cliente:
//!   `{"tipo":"ajuste","volumen":0..100,"brillo":0..100}`  → a la consola
//!
//! `nora-puente seguir` es un cliente hecho para quien solo sabe lanzar un
//! programa y leer sus líneas (la lógica de un extra de Bahía): copia el socket a su
//! salida y su entrada al socket, y si el puente no está lo dice
//! (`{"tipo":"estado","puente":false}`) y vuelve a intentarlo.

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
    matches!(m["tipo"].as_str(), Some("ajuste"))
}

fn cliente(s: UnixStream, estado: Arc<Estado>, a_consola: Arc<Mutex<Sender<String>>>) {
    let Ok(lee) = s.try_clone() else { return };
    // lo que manda el cliente
    thread::spawn(move || {
        for linea in BufReader::new(lee).lines() {
            let Ok(linea) = linea else { break };
            match serde_json::from_str::<Value>(&linea) {
                Ok(m) if permitido(&m) => {
                    let _ = a_consola.lock().unwrap().send(m.to_string());
                }
                Ok(m) => crate::log!("socket local: «{}» no se atiende", m["tipo"].as_str().unwrap_or("?")),
                Err(_) => crate::log!("socket local: línea que no es JSON"),
            }
        }
    });
    // el estado, ahora y en cada cambio
    let mut w = s;
    let (mut c, mut vista) = estado.ver();
    loop {
        if writeln!(w, "{}", c.json()).is_err() {
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

pub fn servir(estado: Arc<Estado>, a_consola: Sender<String>) {
    let ruta = ruta();
    if UnixStream::connect(&ruta).is_ok() {
        crate::log!("socket local: ya hay otro puente en {}; este no lo abre", ruta.display());
        return;
    }
    let _ = std::fs::remove_file(&ruta);
    let escucha = match UnixListener::bind(&ruta) {
        Ok(e) => e,
        Err(e) => {
            crate::log!("socket local: no se puede abrir {}: {e}", ruta.display());
            return;
        }
    };
    let a_consola = Arc::new(Mutex::new(a_consola));
    thread::spawn(move || {
        for s in escucha.incoming().flatten() {
            let (e, a) = (estado.clone(), a_consola.clone());
            thread::spawn(move || cliente(s, e, a));
        }
    });
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
