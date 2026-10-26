# Feedback de harness-directive · 26-10-2026 (producción visual en paralelo)

Una jornada con 8–12 miembros produciendo arte en Blender y la vista de Godot en paralelo. Lo que más tiempo hizo perder fue el harness, no el proyecto. Las «−» son fallas; las «+», prácticas para adoptar.

## FB-067 · − Un candidato aprobado que choca al integrar se rehace desde cero

- kind: fails · cause: guild · status: open · project: harness-directive · by orchestrator · 2026-10-26
- evidence: «día visual de 60 s y granjas» (Kai) pasó los verificadores tres veces; las dos primeras chocó con otra tarea integrada mientras se verificaba y la guild abrió un intento nuevo que volvió a hacer todo el trabajo. Pasó también con «gente, vehículos y drones» (Tomás), «clima: restaurantes» (Fede), «clima: vivienda» (Gala) y «restaurantes y taller como obras» (Orin). En cada caso el orquestador tuvo que escribir a mano «no rehagas: haz git merge de la rama de tu intento y resuelve el conflicto».
- proposal: Ante «accepted but clashes with work integrated meanwhile», la guild intenta primero integrar la rama del candidato sobre el destino actual (merge o rebase). Solo si queda un conflicto textual, abre un intento corto cuya instrucción es exactamente «resuelve este conflicto conservando ambos lados», partiendo de la rama del candidato, no desde cero.


## FB-068 · − Tareas en paralelo que editan el mismo archivo chocan una y otra vez

- kind: fails · cause: guild · status: open · project: harness-directive · by orchestrator · 2026-10-26
- evidence: Cinco tareas agregaron cada una su perfil a tools/blender/entrega_estados.py y chocaron entre sí al integrar; hubo que crear una tarea solo para juntarlas a mano («integrar a mano las cuatro entregas»). Lo mismo con DEFAULT_TESTS de tools/run_tests.sh.
- proposal: Al planear o al empezar un intento, avisar cuando dos tareas abiertas de la misma quest tocan los mismos archivos (por lo que declaran o por lo que ya cambiaron) y ofrecer ponerlas en fila. Para archivos que solo crecen (registros, listas de pruebas), permitir marcar una estrategia de unión al integrar.


## FB-069 · + Notas para toda la quest

- kind: works · cause: guild · status: open · project: harness-directive · by orchestrator · 2026-10-26
- evidence: El diseñador decidió «en esta primera prueba, solo día soleado». Hubo que mandar ocho notas una por una (task note) y aun así quedó fuera una tarea que ya estaba corriendo (Kai re-renderizó 12 climas que no hacían falta).
- proposal: `guild quest note <quest> <texto>`: una restricción que leen todas las tareas abiertas de la quest (las que corren, de inmediato; las pendientes, al empezar) y las que se creen después, hasta que se retire.


## FB-070 · − Un miembro queda trabado en «Submitted» después de `task done`

- kind: fails · cause: guild · status: open · project: harness-directive · by orchestrator · 2026-10-26
- evidence: Las entregas de parque, restaurantes y escuela se integraron a mano y se cerraron con `guild task done --sha`; sus intentos siguieron en Submitted y Alba, Ciro y Heracles aparecen «Submitted on …» y no toman tareas nuevas (las tareas asignadas a ellos quedan «ready»). `guild cancel <attempt>` responde «already submitted a candidate; reject it instead»; `guild_decide reject` funcionó una vez (Dana) y en los otros tres devolvió «the task has no submitted candidate». Desactivar y activar el miembro no los libera.
- proposal: `task done` cierra o rechaza los intentos que la tarea tenía abiertos y libera a sus miembros. Y un comando para liberar a un miembro a la fuerza.


## FB-071 · − No se puede depender de una tarea de otra quest

- kind: fails · cause: guild · status: open · project: harness-directive · by orchestrator · 2026-10-26
- evidence: «calles reales y ciudad muerta en la vista 3D» debía esperar a «megaciudad muerta en manzanas» (otra quest) y «profundidad y máscara de luces» a la prueba de profundidad (otra quest): `depends on unknown task`. Hubo que crear las tareas más tarde a mano o moverlas de quest, y sincronizar ramas con `quest sync`.
- proposal: `--depends` acepta tareas de otra quest del mismo proyecto; al empezar, la guild trae su rama integrada (como quest sync).


## FB-072 · + Despertar al miembro cuando termina el proceso que lanzó

- kind: works · cause: harness · status: open · project: harness-directive · by orchestrator · 2026-10-26
- evidence: Gala lanzó en segundo plano la producción de 15 manzanas (Blender, 2,5 h) y esperó con `sleep 1790`; el render terminó y ella siguió durmiendo hasta que el orquestador la despertó con una nota. El diseñador preguntó por qué «la dormíamos».
- proposal: Un modo de espera del harness: el miembro registra el proceso que lanzó y la guild lo despierta cuando el proceso termina (o cada N minutos con su log), en vez de `sleep` largos a ciegas.


## FB-073 · + Presupuesto común de CPU para el trabajo pesado

- kind: works · cause: guild · status: open · project: harness-directive · by orchestrator · 2026-10-26
- evidence: Llegaron a correr 4 Blender (Cycles), 2 Godot con ventana y las pruebas a la vez en una máquina de 14 núcleos (carga 25). El orquestador fue poniendo «-t 3» o «-t 4» a mano en cada instrucción; un ingeniero corrió Blender con «-t 0» y frenó a todos.
- proposal: Un presupuesto de núcleos por proyecto (o un semáforo de «trabajo pesado») que la guild reparte: cada intento recibe su cuota en una variable de entorno (p. ej. GUILD_THREADS) y las instrucciones pueden usarla.


## FB-074 · + Galería de evidencia publicada por la guild

- kind: works · cause: guild · status: open · project: harness-directive · by orchestrator · 2026-10-26
- evidence: El diseñador decidió todo el arte desde una galería web con cada versión lado a lado con su ficha. El orquestador la armó a mano: extraer cada PNG de la rama, achicarlo, escribir el HTML y publicarlo (unas 70 versiones en dos días).
- proposal: La evidencia de cada tarea (las imágenes de --evidence y sus capturas numeradas) se publica sola en una galería por proyecto, agrupada por quest y tarea, con la referencia declarada al lado y el estado (en curso, aprobada).
