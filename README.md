# Blocko

Editor de escritorio para explorar y editar el Overworld de mundos locales de Minecraft Java Edition 1.21.1 en Windows.

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

## Comprobar v0.3.0 en Windows

Cierra Minecraft y abre copias de Bosque y Pradera guardadas en Java 1.21.1. Para comparar el estado anterior al guardado en Minecraft, usa una copia adicional. Blocko mantiene un bloqueo sobre el mundo abierto.

1. **Pestañas y foco.** Abre ambos mundos desde **Archivo**. Cambia de pestaña y separa Pradera arrastrándola fuera. Comprueba que se conservan la cámara, la selección y el portapapeles compartido. Con **Explorar**, el botón derecho mira de forma temporal y **F** alterna la captura persistente. Con el cursor capturado, usa **2** y **4** para cambiar de herramienta y comprueba que la retícula sigue activa. **WASD** mueve en horizontal, **Espacio** sube y **Q** baja. Con la cámara capturada, cambia de ventana con **Alt+Tab** mientras mantienes **W**. Al volver, comprueba que el cursor está libre y la cámara permanece quieta. Captura otra vez y prueba **Ctrl+C**, **Ctrl+P** y **Ctrl+S** sin que cambie la altura. **Ctrl+P**, los menús y el cambio de foco liberan el cursor. Con una previsualización activa, **Esc** cancela primero la colocación.
2. **Herramientas, paleta y edición.** Pulsa **1** para Explorar, **2** para Selección 3D, **3** para Inspeccionar o **4** para Edición manual. Comprueba el icono activo y su ayuda contextual. Elige **Inspeccionar bloque**, apunta a un bloque y haz clic para fijarlo. Comprueba el ID, la orientación y los detalles de un cofre o cartel. Pulsa **Ctrl+P**, busca «roble» o `minecraft:oak_planks` y elige con flechas y **Enter**. Comprueba el material activo en la barra de estado y pasa el cursor para ver su identificador de Minecraft. Activa **Edición manual** con el lápiz. El clic izquierdo borra el bloque apuntado y el derecho coloca el material adyacente a la cara apuntada. Pulsa **F** para orientar la vista y prueba ambos clics con la retícula. Repite con **Ctrl+Z** y **Ctrl+Y** para comprobar el historial. Comprueba los cambios en el visor y el asterisco de la pestaña.
3. **Rellenar y reemplazar.** Con **Selección 3D**, haz dos clics izquierdos sobre el terreno para marcar A y B. Un tercer clic conserva el volumen. El clic derecho quita B y luego A. **Mayús + clic derecho** limpia ambas. Usa **F** para orientar la vista y el inspector para comprobar o ajustar las coordenadas. Copia con **Ctrl+C** y revisa las dimensiones del portapapeles. En **Editar selección**, elige un material. Usa **Rellenar** o indica el ID de origen para **Reemplazar**. Revisa dimensiones y conteo antes de aplicar. **Ctrl+Z** debe revertir toda la operación y **Ctrl+Y** rehacerla.
4. **Pegado de construcciones.** Copia un volumen de Bosque con una escalera orientada, un cofre y un cartel con texto en ambas caras. En Pradera, **Ctrl+V** activa la previsualización. Prueba destino bajo el cursor y coordenadas precisas, con y sin aire. Cancela con **Esc** o **Retroceso**, luego coloca varias copias con **Mayús + clic izquierdo** y termina con un clic sin modificadores. Cada pulsación crea una operación pendiente.
5. **Guardar y revertir.** Modifica ambos mundos y compara los hashes de sus regiones. Guarda solo Pradera con **Archivo → Guardar** o **Ctrl+S**. Pradera debe perder el asterisco y tener un respaldo. Bosque debe seguir pendiente, con sus regiones intactas. Deshaz el último cambio de Pradera, verifica que vuelve el asterisco y guarda otra vez. Cierra Pradera en Blocko y abre la copia en Minecraft. Comprueba la reversión, la orientación, el inventario y las dos caras del cartel.
6. **Cerrar con cambios e idioma.** Prueba cerrar una pestaña, cerrar una ventana y reemplazar un mundo modificado. **Cancelar** conserva la edición, **Descartar** cierra sin escribir y **Guardar** debe finalizar correctamente antes de cerrar. Comprueba que **1–4** escriben números en un campo de coordenadas o en la búsqueda. **Ctrl+C** y **Ctrl+Z** deben actuar sobre el texto del campo. Repite los menús y la ayuda contextual en inglés desde **Editar → Preferencias**.

### Comparar los archivos antes de guardar

En PowerShell, indica las dos copias abiertas y registra el estado antes de editar. Repite la comparación tras aplicar cambios en Blocko y antes de pulsar Guardar.

```powershell
$copias = @('C:\Mundos de prueba\Bosque', 'C:\Mundos de prueba\Pradera')
function Get-InventarioBlocko {
    Get-ChildItem -LiteralPath $copias -Recurse -File -Force |
        Where-Object { $_.Name -ne 'session.lock' } |
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

Blocko registra las huellas de los archivos al abrir, las comprueba al guardar y toma el bloqueo exclusivo de `session.lock` para la escritura. Guarda los originales de las regiones afectadas en `blocko-backups/pegado-…`, dentro de la copia de destino. Si se interrumpe una escritura, vuelve a abrir esa copia en Blocko antes de cargarla en Minecraft. Si un cambio externo impide la recuperación, la aplicación indica dónde encontrar el respaldo.

El guardado requiere chunks completos de Java 1.21.1 y admite hasta 256 MiB de regiones preparadas por operación. Blocko rechaza cambios que afectarían datos incompatibles, como entidades de bloque fuera del alcance, puntos de interés y chunks externos. Cada copia o edición admite hasta 1 048 576 bloques. Cada documento conserva hasta 64 operaciones de historial o 1 048 576 cambios de bloque mientras está abierto. Puedes deshacer después de guardar y volver a guardar la reversión.

## Traducciones y controles

Añade el idioma a `Language` y `Language::ALL` en `src/i18n.rs`, agrega su nombre y su columna de textos al catálogo y conserva los marcadores `{0}`, `{1}`, etc. Ajusta los mensajes de diagnóstico en `src/diagnostics.rs`. El español es el idioma inicial y sirve de respaldo para traducciones vacías. La preferencia se aplica a todas las ventanas durante la sesión.

Los controles predeterminados y sus contextos viven en `src/actions.rs`. Para probar una reasignación, cambia una combinación en la tabla de `Bindings::default`. La barra inferior usa el mismo registro para mostrar la tecla o gesto. Se conservan los identificadores de acción y el contenido de Minecraft al traducir la interfaz.

## Alcance del visor

Los chunks completos del Overworld se leen bajo demanda y se mallan en segundo plano. Blocko usa `fastnbt` para interpretar NBT y conserva las paletas y propiedades en su modelo interno. El visor aproxima formas y texturas con mallas por chunk y un atlas original. La cámara es libre y atraviesa la geometría.
