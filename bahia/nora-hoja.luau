-- La hoja de Nora (nora-hoja.plm): lo que cuenta nora-puente de la consola. `nora-puente seguir` escribe una
-- línea JSON con el estado al empezar y en cada cambio; lo que se le escribe (un ajuste de volumen o brillo) va a la consola.
local PUENTE = "{{PUENTE}}"
local id = nil
local vol, bri = -1, -1

local function pinta(e)
    fact["puente"] = e.puente == true
    fact["conectada"] = e.conectada == true
    if not e.puente then
        text["estado_txt"] = "Sin puente"
        text["bat_txt"] = "—"
        text["linea1"] = "nora-puente no está"
        text["linea2"] = "systemctl --user start nora-puente"
        return
    end
    if not e.conectada then
        text["estado_txt"] = "Desconectada"
        text["bat_txt"] = "—"
        text["linea1"] = "Buscando la consola…"
        text["linea2"] = "Enchúfala por USB-C"
        return
    end
    local b = tonumber(e.bateria) or -1
    fact["bat"] = b
    fact["cargando"] = e.cargando == true
    text["estado_txt"] = "Conectada"
    text["bat_txt"] = if b >= 0 then string.format("%d %%", b) else "—"
    local app = tostring(e.nombre_app or e.app or "")
    text["linea1"] = if app ~= "" then "En «" .. app .. "»" else "Nora " .. tostring(e.version or "")
    text["linea2"] = (if e.cargando then "Cargando" else "Con batería") .. " · Nora " .. tostring(e.version or "")
    vol, bri = tonumber(e.volumen) or -1, tonumber(e.brillo) or -1
    text["vol_txt"] = if vol >= 0 then tostring(vol) else "—"
    text["bri_txt"] = if bri >= 0 then tostring(bri) else "—"
end

local empezar
empezar = function()
    id = spawn(PUENTE, { "seguir" }, function(linea)
        local ok, e = pcall(json.decode, tostring(linea or ""))
        if ok and type(e) == "table" and e.tipo == "estado" then pinta(e) end
    end, function()
        id = nil
        pinta({ puente = false })
        after(5000, empezar)        -- se fue (o no se encuentra el programa): otra vez dentro de un rato
    end, { stdin = "open" })
end

local function ajuste(clave, valor)
    if not id or valor < 0 then return end
    valor = math.clamp(valor, 0, 100)
    write(id, string.format('{"tipo":"ajuste","%s":%d}\n', clave, valor))
end

on("vol_menos", function() ajuste("volumen", vol - 10) end)
on("vol_mas", function() ajuste("volumen", vol + 10) end)
on("bri_menos", function() ajuste("brillo", math.max(2, bri - 10)) end)
on("bri_mas", function() ajuste("brillo", bri + 10) end)

empezar()
