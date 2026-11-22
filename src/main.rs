//! nora-puente: el servicio del PC que habla con Nora OS (la RG35XX).
//!
//! La consola es el servidor (TCP :7070, un JSON por línea); el puente se
//! conecta a ella, la pone en hora, le manda los datos (usos de Claude y
//! Codex…) y mantiene la conexión viva con latidos. Si se corta, vuelve a
//! intentarlo cada pocos segundos, sin hacer ruido.
//!
//!     nora-puente                 conectarse y quedarse
//!     nora-puente --ip 10.0.0.5   otra consola (o NORA_IP)
//!     nora-puente --usos          imprime las fichas de Usos y sale
//!     nora-puente seguir          cliente del socket local (lo usa el extra de Bahía)
//!     nora-puente bahia           pone (o pone al día) el extra de la barra de Bahía
//!     nora-puente aviso T [texto] manda un aviso a la consola
//!     nora-puente claude-terminado  para el hook `Stop` de Claude Code (su JSON por la entrada)
//!     nora-puente permiso         para el hook `PermissionRequest`: pregunta a la consola (A/B/Y)
//!     nora-puente instalar <carpeta> [--abrir]  pone una app en la consola
//!     nora-puente vigilar <carpeta>             y cada vez que guardas, recarga en caliente
//!     nora-puente apps | borrar <id>            las apps de la consola
//!     nora-puente captura [archivo.png]         la pantalla de la consola
//!     nora-puente pulsar A ABAJO [--ms 90]      pulsar botones de la consola
//!     nora-puente log [-f]                      lo que escriben las apps (nora.log) y sus errores
//!
//! Además abre un socket local para sus caras (ver `local.rs`) y, si usas
//! Bahía, le deja el extra «Nora» de la barra (ver `bahia.rs`).

macro_rules! log {
    ($($t:tt)*) => {
        eprintln!("{} {}", chrono::Local::now().format("%H:%M:%S"), format!($($t)*))
    };
}
pub(crate) use log;

mod apps;
mod avisos;
mod bahia;
mod estado;
mod local;
mod permisos;
mod usos;

use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use estado::Estado;

const PUERTO: u16 = 7070;
const LATIDO: Duration = Duration::from_secs(2);
const SILENCIO: Duration = Duration::from_secs(6);
const REINTENTO: Duration = Duration::from_secs(3);
const USOS_CADA: Duration = Duration::from_secs(60);

/// Lo que los servicios dejan para mandar a la consola. Cada dato lleva una
/// generación: el bucle de la conexión manda los que han cambiado, y todos al
/// conectar (así la consola recupera lo que perdió al reiniciarse).
#[derive(Default)]
struct Datos {
    lista: Vec<(String, u64)>, // (línea JSON, generación), uno por clave
    claves: Vec<String>,
    generacion: u64,
}

impl Datos {
    fn poner(&mut self, clave: &str, linea: String) {
        self.generacion += 1;
        match self.claves.iter().position(|c| c == clave) {
            Some(i) => self.lista[i] = (linea, self.generacion),
            None => {
                self.claves.push(clave.to_string());
                self.lista.push((linea, self.generacion));
            }
        }
    }
    /// las líneas con generación mayor que `desde`
    fn nuevas(&self, desde: u64) -> (Vec<String>, u64) {
        let l = self.lista.iter().filter(|(_, g)| *g > desde).map(|(l, _)| l.clone()).collect();
        (l, self.generacion)
    }
}

type Compartido = Arc<Mutex<Datos>>;

fn dato(app: &str, nombre: &str, valor: Value) -> String {
    json!({"tipo": "dato", "app": app, "nombre": nombre, "valor": valor}).to_string()
}

fn epoch() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Servicio: las fichas de Usos, cada minuto.
fn servicio_usos(datos: Compartido) {
    let mut fallando = false;
    loop {
        match usos::leer() {
            Ok(f) => {
                if fallando {
                    log!("usos: vuelven a leerse");
                }
                fallando = false;
                datos.lock().unwrap().poner("usos.fichas", dato("usos", "fichas", f));
            }
            Err(e) => {
                if !fallando {
                    log!("usos: {e}");
                }
                fallando = true;
            }
        }
        thread::sleep(USOS_CADA);
    }
}

fn mandar(w: &mut TcpStream, linea: &str) -> io::Result<()> {
    w.write_all(linea.as_bytes())?;
    w.write_all(b"\n")
}

fn atender(linea: &[u8], estado: &Estado, esperando: &local::Esperando, oyentes: &local::Oyentes) {
    let Ok(m) = serde_json::from_slice::<Value>(linea) else {
        log!("la consola manda algo que no es JSON: {}", String::from_utf8_lossy(linea));
        return;
    };
    match m["tipo"].as_str().unwrap_or("") {
        "latido" => {}
        "hola" => {
            log!(
                "Nora {} · batería {}%{} · en «{}»",
                m["version"].as_str().unwrap_or("?"),
                m["bateria"],
                if m["cargando"] == true { " (cargando)" } else { "" },
                m["app"].as_str().unwrap_or("?")
            );
            estado.cambiar(|c| c.mezclar(&m));
        }
        "estado" | "ajuste" => estado.cambiar(|c| c.mezclar(&m)),
        "respuesta" => {
            log!("permiso {}: {}", m["id"].as_str().unwrap_or("?"),
                 if m["ok"] == true { if m["siempre"] == true { "permitido siempre" } else { "permitido" } } else { "rechazado" });
            local::responder(esperando, &m);
        }
        "app.instalada" | "app.borrada" | "apps" | "captura" | "app.abriendo" | "app.datos_borrados" | "tema.puesto" => local::responder(esperando, &m),
        "log" | "app.abierta" | "app.error" => {
            if m["tipo"] == "app.error" {
                log!("error en «{}»: {}", m["id"].as_str().unwrap_or("?"),
                     m["error"].as_str().unwrap_or("").lines().next().unwrap_or(""));
            }
            local::difundir(oyentes, &m);
        }
        "accion" => log!("la consola pide «{}» (aún no hace nada)", m["nombre"].as_str().unwrap_or("?")),
        otro => log!("la consola manda «{otro}» (aún no se atiende)"),
    }
}

/// Una conexión con la consola, hasta que se corte.
fn sesion(
    s: TcpStream,
    datos: &Compartido,
    estado: &Estado,
    pedidos: &Receiver<String>,
    esperando: &local::Esperando,
    oyentes: &local::Oyentes,
) -> io::Result<()> {
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(Duration::from_millis(200)))?;
    s.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut w = s.try_clone()?;
    let mut lector = BufReader::new(s);

    mandar(&mut w, &json!({"tipo": "hora", "epoch": epoch()}).to_string())?;
    while pedidos.try_recv().is_ok() {} // lo pedido mientras no estaba, ya no vale
    let mut mandada = 0; // generación de datos ya mandada
    let mut oido = Instant::now();
    let mut latido = Instant::now();
    let mut buf = Vec::new();
    loop {
        let antes = buf.len();
        match lector.read_until(b'\n', &mut buf) {
            Ok(0) => return Err(io::Error::new(ErrorKind::ConnectionAborted, "la consola ha cerrado")),
            Ok(_) => {}
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => return Err(e),
        }
        if buf.len() > antes {
            oido = Instant::now();
        }
        if buf.last() == Some(&b'\n') {
            atender(buf.trim_ascii_end(), estado, esperando, oyentes);
            buf.clear();
        }
        if oido.elapsed() > SILENCIO {
            return Err(io::Error::new(ErrorKind::TimedOut, "la consola no da señales"));
        }
        if latido.elapsed() >= LATIDO {
            latido = Instant::now();
            mandar(&mut w, r#"{"tipo":"latido"}"#)?;
        }
        let (nuevas, g) = datos.lock().unwrap().nuevas(mandada);
        for l in &nuevas {
            mandar(&mut w, l)?;
        }
        mandada = g;
        while let Ok(p) = pedidos.try_recv() {
            mandar(&mut w, &p)?;
        }
    }
}

fn direccion(ip: &str) -> io::Result<SocketAddr> {
    let con_puerto = if ip.contains(':') { ip.to_string() } else { format!("{ip}:{PUERTO}") };
    con_puerto
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(ErrorKind::NotFound, "dirección sin resolver"))
}

const AYUDA: &str = "nora-puente: el PC habla con Nora OS (la RG35XX)

  nora-puente                         el servicio (lo arranca systemd: no hace falta lanzarlo)
  nora-puente --ip 10.0.0.5           el servicio, con otra consola (o NORA_IP)
  nora-puente --usos                  imprime las fichas de Usos y sale

Apps y juegos:
  instalar <carpeta> [--abrir] [--limpiar] [--id X]
                                      pone la app en la consola; dice si abre o su error de Lua (código 3)
  vigilar <carpeta> [--id X]          y la recarga en la consola cada vez que guardas
  abrir <id>                          abre una app de la consola (dice si abre o su error)
  apps                                las apps de la consola
  borrar <id>                         quita una app (sus datos guardados se quedan)
  borrar-datos <id>                   borra los datos guardados de una app (récords, partidas)
  captura [archivo.png]               la pantalla de la consola (por defecto nora-captura.png)
  pulsar A ABAJO … [--ms 90]          pulsa botones, uno tras otro (~0,25 s por orden: no es para tiempo real)
  log [-f] [-n N] [--app id]          lo que escriben las apps (nora.log), aperturas y errores

Temas:
  tema <carpeta> [--poner] [--id X]   instala (o pone al día) un tema; con --poner, lo pone
  temas                               los temas de la consola (y cuál está puesto)
  poner-tema <id>                     pone un tema

Otras:
  aviso <título> [texto]              un aviso en la consola
  bahia                               pone el extra de la barra de Bahía
  seguir                              cliente del socket local (para el extra de Bahía)
  claude-terminado | permiso          hooks de Claude Code (leen su JSON por la entrada)
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some("-h" | "--help" | "help" | "ayuda")) {
        print!("{AYUDA}");
        return;
    }
    if args.first().map(String::as_str) == Some("seguir") {
        local::seguir();
    }
    match args.first().map(String::as_str) {
        Some("aviso") => return avisos::orden_aviso(&args[1..]),
        Some("claude-terminado") => return avisos::orden_claude_terminado(),
        Some("permiso") => return permisos::orden_permiso(),
        Some("instalar") => return apps::orden_instalar(&args[1..]),
        Some("vigilar") => return apps::orden_vigilar(&args[1..]),
        Some("apps") => return apps::orden_apps(),
        Some("borrar") => return apps::orden_borrar(&args[1..]),
        Some("borrar-datos") => return apps::orden_borrar_datos(&args[1..]),
        Some("abrir") => return apps::orden_abrir(&args[1..]),
        Some("tema") => return apps::orden_tema(&args[1..]),
        Some("temas") => return apps::orden_temas(),
        Some("poner-tema") => return apps::orden_poner_tema(&args[1..]),
        Some("captura") => return apps::orden_captura(&args[1..]),
        Some("pulsar") => return apps::orden_pulsar(&args[1..]),
        Some("log") => return apps::orden_log(&args[1..]),
        _ => {}
    }
    if args.first().map(String::as_str) == Some("bahia") {
        match bahia::instalar() {
            Ok(Some(r)) => println!("extra de Bahía puesto en {}", r.display()),
            Ok(None) => println!("el extra ya estaba al día (o no hay Bahía)"),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }
    // el servicio solo con lo suyo: cualquier otra cosa no debe arrancar un segundo puente
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--usos" => {}
            "--ip" => i += 1,
            otra => {
                eprintln!("nora-puente: no conozco «{otra}»\n\n{AYUDA}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    if args.iter().any(|a| a == "--usos") {
        match usos::leer() {
            Ok(f) => println!("{}", serde_json::to_string_pretty(&f).unwrap()),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }
    let ip = args
        .iter()
        .position(|a| a == "--ip")
        .and_then(|i| args.get(i + 1).cloned())
        .or_else(|| std::env::var("NORA_IP").ok())
        .unwrap_or_else(|| "192.168.7.2".into());
    let destino = match direccion(&ip) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("nora-puente: {ip}: {e}");
            std::process::exit(2);
        }
    };

    match bahia::instalar() {
        Ok(Some(r)) => log!("Bahía: extra «Nora» de la barra puesto en {}", r.display()),
        Ok(None) => {}
        Err(e) => log!("Bahía: no se pudo poner el extra de la barra: {e}"),
    }
    let estado = Arc::new(Estado::nuevo());
    let (a_consola, pedidos) = mpsc::channel();
    {
        let a = a_consola.clone();
        thread::spawn(move || avisos::servicio_notificaciones(a));
    }
    let esperando: local::Esperando = Arc::default();
    let oyentes: local::Oyentes = Arc::default();
    if !local::servir(estado.clone(), a_consola, esperando.clone(), oyentes.clone()) {
        // dos puentes se quitarían la consola el uno al otro cada pocos segundos
        eprintln!("nora-puente: ya hay un puente corriendo (systemctl --user status nora-puente); este no hace falta");
        std::process::exit(1);
    }

    let datos: Compartido = Arc::default();
    {
        let d = datos.clone();
        thread::spawn(move || servicio_usos(d));
    }

    log!("nora-puente {}: buscando a Nora en {destino}", env!("CARGO_PKG_VERSION"));
    let mut avisado = false; // ya se dijo que no está: no repetirlo cada 3 s
    loop {
        match TcpStream::connect_timeout(&destino, Duration::from_secs(2)) {
            Ok(s) => {
                log!("conectado a Nora");
                avisado = false;
                estado.cambiar(|c| {
                    c.conectada = true;
                    c.ip = destino.ip().to_string();
                });
                if let Err(e) = sesion(s, &datos, &estado, &pedidos, &esperando, &oyentes) {
                    log!("desconectado: {e}");
                }
                estado.cambiar(|c| c.conectada = false);
                local::sin_consola(&esperando);
            }
            Err(e) => {
                if !avisado {
                    log!("Nora no está ({e}); se sigue intentando");
                    avisado = true;
                }
            }
        }
        thread::sleep(REINTENTO);
    }
}
