# Blocko

Editor de escritorio para explorar el Overworld y copiar construcciones entre mundos locales de Minecraft Java Edition 1.21.1 en Windows.

## Ejecutar en Windows

Usa Rust estable con el toolchain MSVC, las herramientas de compilación de C++ y el SDK de Windows. Abre PowerShell en la carpeta que contiene `Cargo.toml`.

```powershell
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $env:Path = (Join-Path $env:USERPROFILE '.cargo/bin') + ';' + $env:Path
}
cargo run --release --locked
```

Elige la carpeta que contiene `level.dat` y `region`. También puedes pasarla al iniciar la aplicación.

```powershell
cargo run --release --locked -- "C:\Mundos de prueba\Bosque"
```

Si Cargo muestra `link.exe not found`, instala **Desarrollo para el escritorio con C++** en Visual Studio 2022, con MSVC y un SDK de Windows. Ejecuta el comando desde **x64 Native Tools Command Prompt for VS 2022**. [Requisitos de Rust para Windows](https://learn.microsoft.com/en-us/windows/dev-environment/rust/setup)

## Comprobar v0.2.0 en Windows

Cierra Minecraft y usa copias de Bosque y Pradera guardadas en Java 1.21.1. Conserva los respaldos mientras compruebas las escrituras.

1. **Visor y pestañas.** Abre Bosque con **Abrir mundo** y usa **+** en la fila de pestañas para abrir Pradera. Cambia de pestaña después de mover la cámara o marcar una esquina en cada mundo. Ambos deben conservar su posición y selección. Arrastra la pestaña Pradera más allá del borde de la ventana y suelta. Pradera debe aparecer en su propia ventana y Bosque debe seguir abierto. Prueba cerrar una pestaña, cerrar una ventana y volver a abrir un mundo. El menú **Archivo** permite reemplazar el mundo de la pestaña activa, cerrarla y abrir directamente en otra ventana. Intenta volver a abrir una carpeta que ya tenga una pestaña.
2. **Cámara y selección.** Mantén pulsado el botón derecho sobre el terreno para mirar. La última fila reserva espacio para los comandos contextuales y muestra los controles de vuelo durante la captura del cursor. En la barra izquierda, elige **Selección 3D**. En el inspector, pulsa **Marcar A/B** y selecciona dos bloques. Comprueba el contorno, las marcas y las dimensiones inclusivas. Ajusta X, Y o Z y copia con **Editar → Copiar selección** o **Ctrl+C**. En un campo de coordenadas con texto seleccionado, Ctrl+C debe copiar el texto a Windows y conservar la construcción compartida. Abre **Portapapeles compartido** en ambas ventanas para comparar el origen y las dimensiones.
3. **Previsualización y cancelación.** En Pradera, elige **Editar → Pegar construcción** o pulsa **Ctrl+V**. Mueve el cursor por caras del terreno y revisa la esquina mínima mostrada en el inspector. Elige **Coordenadas** o edita X, Y y Z para una colocación precisa. Prueba **Con aire** y **Sin aire**. Gira la cámara con el botón derecho y comprueba que la previsualización sigue activa. Cancela desde **Editar → Cancelar operación** y repite con **Esc** y **Retroceso**. La copia debe seguir disponible y los hashes del mundo deben coincidir. En un campo de texto y en el diálogo de apertura, comprueba que las teclas se aplican al control que tiene el foco.
4. **Guardar una vez y repetir.** Apunta a un lugar válido de una copia de Pradera. Mantén pulsado el botón izquierdo y comprueba que se guarda una sola copia. Vuelve a iniciar la vista previa y coloca tres copias en lugares distintos con **Mayús + clic izquierdo**. Termina con un clic sin modificadores. Revisa un respaldo por operación y la ubicación de los bloques en el destino. Repite con varias pestañas y dos ventanas. Para revisar el destino y las regiones antes de guardar, usa **Preparar** y **Confirmar pegado**.
5. **Datos de Minecraft.** Copia un volumen con una escalera orientada, un cofre simple con objetos y un cartel escrito por ambos lados. Pruébalo en dos copias de destino, una con cada modo de aire. Cierra el destino en Blocko y ábrelo en Minecraft Java 1.21.1. Comprueba la orientación, el inventario, ambas caras del cartel y los huecos.
6. **Estados e idioma.** Prueba una carpeta incompatible, un mundo en uso por Minecraft y un destino inválido. Verifica el nombre del mundo, el mensaje y los detalles disponibles. Cambia a **English** desde **Editar → Preferencias** con dos ventanas abiertas, reduce el ancho y cambia de pestaña. Usa **Ver** para ocultar o recuperar las herramientas y el inspector. Consulta las combinaciones activas en **Ayuda → Controles y atajos**. Revisa menús, estados, inspector y controles de la barra inferior en ambos idiomas. Los nombres de mundo, los datos del portapapeles y la edición activa deben conservarse.
7. **Terreno.** Compara el mismo punto y orientación en Bosque y en Pradera con las capturas del MVP. Comprueba nieve, vegetación, elevaciones y carga progresiva en ambos mundos. En una zona con agua y hielo, revisa sus límites con el terreno a corta y media distancia. Haz la misma pasada con dos ventanas y compara la fluidez y el consumo con el Administrador de tareas.

### Verificar que cancelar conserva los archivos

Toma un inventario de archivos antes de explorar, copiar, previsualizar y cancelar. Cierra Blocko antes de comparar.

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

Si los inventarios coinciden, `Compare-Object` no imprime nada.

### Pruebas de desarrollo

```powershell
cargo fmt --all -- --check
cargo check --all-targets --locked
cargo test --release --locked
$env:BLOCKO_TEST_WORLDS = 'C:\Mundos de prueba'
cargo test --release --locked -- --include-ignored
cargo run --release --locked -- --inspeccionar "C:\Mundos de prueba\Bosque"
```

Las pruebas ignoradas buscan **Bosque** y **Pradera** bajo `BLOCKO_TEST_WORLDS`. Requieren los fixtures de Java 1.21.1 que incluyen la selección de Bosque entre `(14, 66, -50)` y `(17, 73, -47)` y el cofre en `(86, -23, 119)`. La prueba de escritura prepara su propia copia de Pradera en el directorio temporal de Windows y conserva allí los respaldos.

`cargo run --release --locked -- --demo` abre una escena pequeña para comprobar cámaras independientes y estado compartido sin cargar un mundo.

## Guardado y recuperación

Blocko verifica las huellas de los archivos al abrir, preparar y confirmar, y toma el bloqueo exclusivo de `session.lock` al guardar. Guarda los originales de las regiones afectadas en `blocko-backups/pegado-…`, dentro de la copia de destino. Si se interrumpe una escritura, vuelve a abrir esa copia en Blocko antes de cargarla en Minecraft. Si un cambio externo impide la recuperación, la aplicación indica dónde encontrar el respaldo.

El guardado requiere chunks completos de Java 1.21.1 y admite hasta 256 MiB de regiones preparadas por operación. Blocko rechaza cambios que afectarían datos incompatibles, como entidades de bloque fuera del alcance, puntos de interés y chunks externos. La copia admite hasta 1 048 576 bloques.

## Traducciones y controles

Añade el idioma a `Language` y `Language::ALL` en `src/i18n.rs`, agrega su nombre y su columna de textos al catálogo y conserva los marcadores `{0}`, `{1}`, etc. Ajusta los mensajes de diagnóstico en `src/diagnostics.rs`. El español es el idioma inicial y sirve de respaldo para traducciones vacías. La preferencia se aplica a todas las ventanas durante la sesión.

Los controles predeterminados y sus contextos viven en `src/actions.rs`. Para probar una reasignación, cambia una combinación en la tabla de `Bindings::default`. La barra inferior usa el mismo registro para mostrar la tecla o gesto. Se conservan los identificadores de acción y el contenido de Minecraft al traducir la interfaz.

## Alcance del visor

Los chunks completos del Overworld se leen bajo demanda y se mallan en segundo plano. Blocko usa `fastnbt` para interpretar NBT y conserva las paletas y propiedades en su modelo interno. El visor aproxima formas y texturas con mallas por chunk y un atlas original. La cámara es libre y atraviesa la geometría.
