# Blocko

Visor de escritorio para explorar el Overworld de copias locales de Minecraft Java Edition 1.21.1 en Windows.

## Ejecutar en Windows

Usa Rust estable con el toolchain MSVC, las herramientas de compilación de C++ y el SDK de Windows. Abre PowerShell en la carpeta que contiene `Cargo.toml`.

```powershell
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $env:Path = (Join-Path $env:USERPROFILE '.cargo/bin') + ';' + $env:Path
}
cargo run --release
```

**Abrir mundo** carga una copia en la ventana actual. **Abrir en otra ventana** conserva el mundo y la cámara que ya estaban abiertos. Elige la carpeta que contiene `level.dat` y `region`. También puedes pasar esa carpeta en la línea de comandos.

```powershell
cargo run --release -- "C:\Mundos de prueba\Bosque"
```

Mantén pulsado el botón derecho sobre el visor para mirar con el ratón. Usa **WASD** para desplazarte, **Espacio** y **Ctrl izquierdo** para subir y bajar, y **Mayús** para acelerar. Suelta el botón o pulsa **Esc** para liberar el cursor.

### Si Cargo muestra `link.exe not found`

En Visual Studio Installer, elige **Modificar** en Visual Studio 2022 y agrega **Desarrollo para el escritorio con C++**. Confirma que queden seleccionados **MSVC v143 para x64/x86** y un **SDK de Windows 10 u 11**. Después abre **x64 Native Tools Command Prompt for VS 2022** desde Inicio y ejecuta estos comandos desde el repositorio. [Requisitos de Rust para Windows](https://learn.microsoft.com/en-us/windows/dev-environment/rust/setup)

```cmd
where link
cargo run --release
```

## Comprobar BLCK-1

1. Con Minecraft cerrado, prepara copias de Bosque y Pradera. Abre Bosque y comprueba que el nombre coincide con el mostrado en Minecraft. Recorre varios chunks y comprueba que el terreno se completa a medida que avanzas.
2. Abre Pradera en otra ventana. Desplázate en Pradera y vuelve a Bosque. Su cámara debe seguir en las mismas coordenadas y con la misma orientación.
3. En una de las ventanas, cambia al otro mundo y comprueba que el título y la cámara se actualizan. Cierra esa ventana y confirma que puedes seguir explorando en la restante. Repite cerrando primero la ventana inicial.
4. Intenta abrir una copia que ya esté abierta en Minecraft y un mundo de otra versión. Blocko debe mostrar el error en la ventana que inició la apertura y conservar el mundo que ya mostraba.

Para comprobar la lectura y el mallado inicial sin abrir una ventana, ejecuta el siguiente comando con cada copia. El resultado incluye el nombre, el punto de inicio y el número de vértices del primer chunk.

```powershell
cargo test
cargo run --release -- --inspeccionar "C:\Mundos de prueba\Bosque"
cargo run --release -- --inspeccionar "C:\Mundos de prueba\Pradera"
```

Para verificar que una exploración no modifica las copias, toma un inventario de rutas y hashes antes y después. Ejecuta el primer bloque antes de abrirlas y el segundo después de cerrar Blocko.

```powershell
$copias = @('C:\Mundos de prueba\Bosque', 'C:\Mundos de prueba\Pradera')
function Get-InventarioBlocko {
    Get-ChildItem -LiteralPath $copias -Recurse -File -Force |
        Get-FileHash -Algorithm SHA256 |
        Sort-Object Path |
        ForEach-Object { '{0}  {1}' -f $_.Hash, $_.Path }
}
$antes = Get-InventarioBlocko
```

```powershell
Compare-Object $antes (Get-InventarioBlocko)
```

Si los inventarios coinciden, `Compare-Object` no imprime nada. Para probar las cámaras y el estado compartido con dos escenas pequeñas, ejecuta `cargo run -- --demo` y usa **Otra ventana de prueba** y **Compartir selección ficticia**.

## Alcance del visor

Los chunks completos del Overworld se leen bajo demanda y se mallan en segundo plano. El lector usa `fastnbt` para interpretar NBT y transmite al modelo interno las paletas y propiedades de bloques. El visor aproxima las formas y texturas con mallas por chunk y un atlas original. Algunas formas, materiales y efectos de Minecraft se simplifican. La cámara es libre y atraviesa la geometría.
