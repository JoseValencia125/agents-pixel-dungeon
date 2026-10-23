# Feedback de harness-directive · 23-10-2026 (estrategia visual)

Qué funcionó para iterar lo visual rápido y qué lo frenó. Las «+» son prácticas para adoptar en roles, brief y guild.

## FB-056 · − La pestaña Gremio mezcla miembros activos, libres y desactivados en una sola lista

- kind: fails · cause: guild · status: open · project: harness-directive · by user · 2026-10-23T18:51:04.180Z
- evidence: Captura del 2026-10-23 15:50 (Gremio · 15): los 18 miembros aparecen intercalados por orden de creación. Los que trabajan (Alba, Ciro, Eva, Heracles, Brecha, Vesna), los que tienen candidato listo (Bruno, Orin), los libres (Dana, Fede, Gala, Tomás, Kai, Oro, Marga) y los desactivados (Nadia, Teseo, Perseo). Para ver quién está trabajando hay que leer fila por fila; el punto de color es lo único que los distingue.
- proposal: Agrupar la lista por estado, con encabezado y conteo por grupo: Trabajando / Esperando decisión → Libres → Desactivados (plegado por defecto). Dentro de cada grupo, orden estable por nombre o por actividad reciente.


## FB-057 · + Trabajo visual EN CONTEXTO: construir el asset dentro de la escena real del juego, no aislado

- kind: works · cause: role · status: open · project: harness-directive · by orchestrator · 2026-10-23T20:12:01.139Z
- evidence: Ciro y Eva modelaron edificios aislados sobre fondo vacío y salieron cajas con cubos de alambre que no parecían del juego (2 tareas tiradas). Alba hizo la prueba del día dentro del prototipo completo (git checkout 91de469 -- .) y acertó a la primera. Propuesta: el rol asset-artist/technical-artist exige trabajar y capturar dentro de la escena del juego, con la cámara del juego, y comparar con la referencia.


## FB-058 · + Tareas de prueba cortas (spike) que no se integran

- kind: works · cause: guild · status: open · project: harness-directive · by orchestrator · 2026-10-23T20:12:01.242Z
- evidence: La prueba «ciudad de día sin lluvia» (30 min, sin integrar) dio al diseñador una decisión visual en minutos. Propuesta: un tipo de tarea spike con tope de tiempo, aceptación sin integrar por defecto y sus imágenes como entregable.


## FB-059 · + Evidencia en vivo: capturas por paso en una carpeta que el diseñador ve

- kind: works · cause: guild · status: open · project: harness-directive · by orchestrator · 2026-10-23T20:12:01.348Z
- evidence: Pedir a los artistas una captura numerada por paso en una carpeta del diseñador permitió corregir antes de que terminaran. Hoy lo hago con un script que copia desde los worktrees. Propuesta: canal de evidencia en vivo por tarea (las imágenes aparecen en la app mientras el miembro trabaja).


## FB-060 · + Pedir una imagen de referencia y archivos exactos en cada tarea visual

- kind: works · cause: orchestrator · status: open · project: harness-directive · by orchestrator · 2026-10-23T20:12:01.455Z
- evidence: Las tareas visuales que nombraron la referencia (docs/visual_ref/...png) y los archivos a reutilizar del historial (git show 91de469:<ruta>) acertaron; las que decían «trae el look» reinventaron. Propuesta: plantilla de tarea visual con campos obligatorios: referencia, archivos fuente, comparación lado a lado.


## FB-061 · − Cancelar una tarea en curso la deja Blocked y hay que cancelarla dos veces

- kind: fails · cause: guild · status: open · project: harness-directive · by orchestrator · 2026-10-23T20:12:01.560Z
- evidence: e-vivienda-agua y e-energia-trat: el primer cancel dejó el intento cancelado pero la tarea en Blocked («retry or cancel») y apareció como «NEEDS YOU» para el usuario; hubo que repetir cancel. Propuesta: cancel de la tarea termina la tarea en un paso.


## FB-062 · − Recrear algo borrado: la guild no sugiere los archivos del historial

- kind: fails · cause: guild · status: open · project: harness-directive · by orchestrator · 2026-10-23T20:12:01.663Z
- evidence: Se pidió volver al look del prototipo y los miembros reconstruyeron desde cero; todo estaba en git (91de469). Propuesta: cuando una tarea menciona algo que existió, la guild busca en el historial (archivos borrados, docs/REMOVED_*.md) y adjunta las rutas al brief.


## FB-063 · − El orquestador de la guild escala permisos inofensivos que debería decidir solo; no hay política por proyecto

**Postura del usuario (23-10-2026): «100 %: el orquestador solo debe escalar lo extremadamente peligroso».**

**Evidencia:** en una tarde, el orquestador (Oro) escaló al humano decenas de permisos: leer referencias fuera del worktree, `git add`/`commit` en el propio worktree, `git fetch`, `web_search` de APIs de Godot, importar el proyecto. Dana esperó 45 min y Alba 26 min con el trabajo detenido. El delegado terminó respondiendo a mano lo que el orquestador debía decidir.

**Propuesta:** invertir el criterio por defecto: el orquestador aprueba todo lo que se queda dentro del worktree, del scratch de la tarea, de lecturas del repo y de rutas de referencia declaradas, git en la propia rama, red de solo lectura (fetch, búsqueda de documentación) y herramientas del proyecto. Escala solo lo extremadamente peligroso: borrar fuera del worktree, escribir en el checkout del usuario, push a ramas compartidas, credenciales, red con escritura, comandos destructivos del sistema. Una política por proyecto en `.guild/guild.toml` (rutas de lectura, comandos permitidos) para ajustar el resto.
