//! Cómo está la consola, según lo último que ha dicho. Lo leen las caras del
//! puente (el extra de Bahía…) por el socket local, y se les avisa al cambiar.

use std::sync::{Condvar, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

#[derive(Default, Clone, PartialEq)]
pub struct Consola {
    pub conectada: bool,
    pub ip: String,
    pub version: String,
    pub bateria: i64, // -1: no se sabe
    pub cargando: bool,
    pub app: String,  // id de la app abierta
    pub nombre_app: String,
    pub volumen: i64, // 0..100, -1: no se sabe
    pub brillo: i64,
}

impl Consola {
    pub fn json(&self) -> Value {
        json!({
            "tipo": "estado",
            "puente": true,
            "conectada": self.conectada,
            "ip": self.ip,
            "version": self.version,
            "bateria": self.bateria,
            "cargando": self.cargando,
            "app": self.app,
            "nombre_app": if self.nombre_app.is_empty() { &self.app } else { &self.nombre_app },
            "volumen": self.volumen,
            "brillo": self.brillo,
            // la píldora de la barra de Bahía (extras/GUIDE.md de Bahía)
            "bar": {
                "show": true,
                "text": if !self.conectada { "—".to_string() }
                        else if self.bateria >= 0 { format!("{} %", self.bateria) }
                        else { "Nora".to_string() },
                "alert": self.conectada && self.bateria >= 0 && self.bateria < 15 && !self.cargando,
            },
        })
    }

    /// Lo que la consola cuenta de sí misma (`hola`, `estado`, `ajuste`).
    pub fn mezclar(&mut self, m: &Value) {
        if let Some(v) = m["version"].as_str() {
            self.version = v.into();
        }
        if let Some(v) = m["bateria"].as_i64() {
            self.bateria = v;
        }
        if let Some(v) = m["cargando"].as_bool() {
            self.cargando = v;
        }
        if let Some(v) = m["app"].as_str() {
            self.app = v.into();
            self.nombre_app = m["nombre_app"].as_str().unwrap_or("").into();
        }
        if let Some(v) = m["volumen"].as_i64() {
            self.volumen = v;
        }
        if let Some(v) = m["brillo"].as_i64() {
            self.brillo = v;
        }
    }
}

/// El estado y su número de versión, con aviso a quien espera un cambio.
pub struct Estado {
    dentro: Mutex<(Consola, u64)>,
    cambio: Condvar,
}

impl Estado {
    pub fn nuevo() -> Self {
        let c = Consola { bateria: -1, volumen: -1, brillo: -1, ..Default::default() };
        Estado { dentro: Mutex::new((c, 1)), cambio: Condvar::new() }
    }

    pub fn ver(&self) -> (Consola, u64) {
        self.dentro.lock().unwrap().clone()
    }

    pub fn cambiar(&self, f: impl FnOnce(&mut Consola)) {
        let mut g = self.dentro.lock().unwrap();
        let antes = g.0.clone();
        f(&mut g.0);
        if g.0 != antes {
            g.1 += 1;
            self.cambio.notify_all();
        }
    }

    /// Espera a que la versión pase de `vista` (o a que pase `max`).
    pub fn esperar(&self, vista: u64, max: Duration) -> (Consola, u64) {
        let g = self.dentro.lock().unwrap();
        let (g, _) = self.cambio.wait_timeout_while(g, max, |g| g.1 == vista).unwrap();
        g.clone()
    }
}
