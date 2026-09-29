# Revisión del diseño

El [README](../README.md) contiene el diseño y el plan. Este documento recoge solo el porqué de las decisiones, las recomendaciones pendientes y algunas aclaraciones. Última revisión: 2026-09-30.

## Puntos fuertes del diseño

- **Base Bevy fijada:** es lo que hace viable el proyecto para una persona.
- **Definición / spawn / estado runtime / save separados:** permiten hot reload sobre instancias vivas y saves robustos.
- **Descarga ≠ muerte ≠ eliminación, y streaming independiente de las relaciones:** evitan bugs como el del esbirro que muere al descargarse la celda de su jefe.
- **Reacciones encoladas:** encajan con `Commands` y los observers de Bevy.
- **Simulación separada de presentación:** sirve de base para multiplayer futuro y tests headless.
- **"Open in…" y Blender headless** en lugar de reimplementar herramientas.
- **Playground concreto y non-goals explícitos.**

## Riesgos principales

1. **Animación procedural.** Las poses base reducen mucho el riesgo, pero sigue siendo lo más incierto del motor. Por eso el hito 0 incluye un spike de 2–3 semanas: si no convence, se replantea antes de construir encima. El toolbox debe editar el mismo formato que lee el runtime.
2. **Alcance.** El plan mantiene fuera de la v1 la web, otros SO, multiplayer, pintura de texturas y vídeo. Cada uno de ellos basta para descarrilar el proyecto si entra antes de tiempo.

## Porqué de las decisiones

**Linaje y tutela separados.** Un ogro adoptado es a la vez descendiente de `minions/ogre` y pupilo del jugador. Con una sola relación de un padre, adoptarlo le haría perder su tipo. Además, el tipo nunca cambia en runtime y la tutela sí.

**Capacidades declaradas en el maestro.** Lo concedido depende del maestro: el mismo ogro gana `fetch` con el jugador y `rally` con el jefe. Declararlo en el pupilo obligaría a listar todos sus posibles maestros. El filtro por linaje hace que `small_ogre` herede lo concedido a `ogre`. Detalle de implementación: registrar qué fue concedido, para no quitar al terminar la relación un componente que el pupilo ya tenía por sí mismo. Se implementa con los hooks de insert/remove de la relación en Bevy.

**Anclaje como constraint, no como relación.** Las constraints ya tienen objetivo, peso y prioridad. Tratar el anclaje como una más da transiciones suaves (agarrar = subir el peso de 0 a 1) y lo unifica con los grips. Un anclaje rígido de peso 1 puede usar por dentro la jerarquía de transforms de Bevy.

**Sin `spawnedBy`.** A la jugabilidad no le interesa quién creó una entidad. Si importa, el spawner asigna `masterIs`. El registro de qué ha generado cada spawner es contabilidad interna para el save.

**IDs: ruta para lo autorado, UUID para lo generado.** Las rutas son legibles y dan diffs limpios. Su punto débil es renombrar (ver pendientes).

**Binario por offsets, diseñado tarde.** Es rápido y encaja con el streaming, sobre todo para datos grandes y estáticos: mallas, poses, navmesh, bundles. Congelar el layout mientras los schemas cambian cada día obligaría a rehacerlo continuamente. Antes de escribirlo desde cero, evaluar `rkyv`, que ya hace acceso por offsets relativos con validación.

**Paquete = componentes + acciones + sistemas.** Las acciones cubren lo discreto. La flotación o la gravedad se ejecutan cada tick, y eso son sistemas. Sin ABI estable, un paquete solo puede ser un crate.

**Gravedad por campos.** Con la gravedad global de la librería de física desactivada, la escena y los planetas son el mismo `GravityField` con distinto volumen y dirección. La suma de campos da además el "arriba" local al controller y a la animación, y sacar la gravedad planetaria a un paquete externo valida el sistema de paquetes.

**Invariantes de multiplayer desde el día 1.** Timestep fijo, IDs estables, input como comandos, simulación sin lecturas de presentación y aleatoriedad con semilla. Cuestan poco al principio y retroadaptarlos es caro.

## Recomendaciones pendientes

Corresponden a las decisiones abiertas del README.

| Tema | Recomendación | Motivo |
| --- | --- | --- |
| Acciones con duración | Acciones instantáneas; lo que dura es un componente de estado (`Dying { timer }`) que invoca otra acción al terminar. Un sistema compartido llama a `die` cuando `Health <= 0` | Define sin ambigüedad qué es "completar" y evita que alguna ruta de muerte se salte `die` |
| Coordenadas de spawner | Posición en coordenadas de mundo o de zona; el compilador deriva la celda | Con `tile` en los datos, cambiar el tamaño de celda rompe todos los spawners |
| Pupilos sin maestro | Quedan huérfanos por defecto; el maestro declara reacciones si quiere otra cosa (morir, reasignarse) | Es lo menos sorprendente y reutiliza las reacciones |
| Tipo primordial obligatorio | Sí: todo linaje termina en un primordial (`Actor`, `Terrain`…) | Da a los sistemas del motor (sensing, save, editor) una base común garantizada |
| Renombrar rutas | El editor actualiza todas las referencias y registra un alias `antigua → nueva` para los saves | Es barato y no rompe las partidas |
| UI del editor | egui (`bevy_egui`) con tema propio | La interoperabilidad con C no depende de la UI. egui se integra con el viewport de Bevy, trae inspector y admite un estilo completamente propio (Rerun es un ejemplo). imgui no aporta nada extra y gpui es difícil de combinar con el viewport. Falta valorar "pocketJS" (enlace pendiente) |
| Scripting | Solo Rust: acciones parametrizadas desde datos más hotpatching de Bevy (`subsecond`, se valida en el spike) | Un lenguaje runtime duplicaría el sistema de acciones y el editor |
| IA | Behavior tree (con estado "running") en lugar de un árbol de decisión puro; boids para bancos de peces | "Caminar hasta X" dura varios ticks |

**Librerías a evaluar** frente a Bevy 0.19.1:
- **Física:** Avian (ECS nativo, incluye interpolación).
- **Input:** `bevy_enhanced_input` o `leafwing-input-manager`.
- **Navmesh:** `vleue_navigator` u `oxidized_navigation`.
- **Audio:** `bevy_kira_audio` o `bevy_seedling`.
- **Binario:** `rkyv`.
- **JSONC con CST:** `jsonc-parser` (dprint).

Bevy no reproduce vídeo; si hiciera falta, habría que integrar un decoder.

## Aclaraciones

**Interpolación.** La simulación corre a paso fijo (por ejemplo 60 Hz) y la pantalla a otra frecuencia. Si se dibuja el último tick, el movimiento da tirones. Interpolar es dibujar una mezcla entre el tick anterior y el actual según el tiempo transcurrido. Solo afecta a lo visual, y es el mismo mecanismo que el multiplayer usará para las entidades remotas.

**Migración de datos.** Si `"Health": { "hp": 100 }` pasa a `{ "current": 100, "max": 100 }`, los JSONC y los saves existentes dejan de cargar. Migrar consiste en versionar cada fichero y tener funciones `v3 → v4` que lo reescriben conservando los comentarios. Mientras no haya partidas de jugadores, basta con un error claro (`ogre/entity.jsonc:12: campo desconocido "hp" en Health`) y corregir a mano.

**Undo/redo.** Solo hay dos casos que no son obvios:
- Si un fichero cambia fuera del editor (por "Open in…"), se invalidan las entradas de ese fichero.
- Volcar el historial a disco sirve para recuperarse tras un crash, pero es opcional.
