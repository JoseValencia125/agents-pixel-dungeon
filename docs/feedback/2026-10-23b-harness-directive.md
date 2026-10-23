# Feedback de harness-directive · 23-10-2026 (tarde)

Lecciones de la ronda visual neón y de la ronda 3 de simulación. Cada una con evidencia y propuesta.

## FB-049 · − Una tarea que borra cientos de archivos se integra sin mostrar al humano la lista de lo borrado

- kind: fails · cause: guild · status: open · project: harness-directive · by orchestrator · 2026-10-23T18:29:04.261Z
- evidence: La limpieza DEC-075 (cae72a4) borró 631 archivos y 56 598 líneas, incluido todo el look visual que el diseñador quería conservar; se aceptó sin que nadie viera la lista. Propuesta: si un candidato borra más de N archivos o líneas, la guild lo escala con el manifiesto agrupado (código, escenas, shaders, assets) y pide confirmación por grupo.


## FB-050 · − La guild no muestra las capturas: el orquestador y el usuario no pueden juzgar trabajo visual

- kind: fails · cause: guild · status: open · project: harness-directive · by orchestrator · 2026-10-23T18:29:04.370Z
- evidence: En las rondas visuales, el orquestador escaló TODAS las entregas con «no puedo ver las capturas, el diff está cortado»; captures/ está en .gitignore y el verificador de capturas apunta a tools/capture.sh que ya no existe. Hubo que copiar PNG a mano desde los worktrees. Propuesta: que la tarea declare sus imágenes de evidencia, la guild las adjunte al candidato y las muestre en la app y al orquestador (que pueda mirarlas).


## FB-051 · − Las notas a una tarea en curso llegan solo al final del turno: no sirven para corregir el rumbo

- kind: fails · cause: guild · status: open · project: harness-directive · by orchestrator · 2026-10-23T18:29:04.477Z
- evidence: Alba (n-base) siguió reinventando shaders durante su turno aunque se le mandaron tres notas con la lista exacta de archivos a portar; las leyó al terminar. Propuesta: entregar la nota en la siguiente vuelta de herramienta del miembro (como un mensaje), o una acción «pausar y redirigir».


## FB-052 · − No hay rutas de lectura permitidas por quest: cada referencia fuera del worktree pide permiso

- kind: fails · cause: guild · status: open · project: harness-directive · by orchestrator · 2026-10-23T18:29:04.582Z
- evidence: Alba, Dana y Heracles pidieron permiso uno por uno para leer las capturas de referencia en ~/harness-directive-capturas; always-accept no se recuerda en Kiro (FB-018). Hubo que copiar las referencias al repo. Propuesta: la quest declara rutas de solo lectura permitidas.


## FB-053 · − Cancelar una tarea bloquea a sus dependientes sin forma de reapuntarlos

- kind: fails · cause: guild · status: open · project: harness-directive · by orchestrator · 2026-10-23T18:29:04.691Z
- evidence: Al reemplazar n-base por n-base2 y luego por n-restaurar, sus 4–5 dependientes quedaron Blocked («dependency failed») y hubo que cancelarlos y recrearlos copiando las instrucciones. Propuesta: guild task set <id> --depends para reapuntar, o «reemplazar tarea» que mantenga los dependientes.


## FB-054 · − Cambiar el harness de un miembro no afecta su intento en curso, y no se puede reiniciar una tarea Running

- kind: fails · cause: guild · status: open · project: harness-directive · by orchestrator · 2026-10-23T18:29:04.798Z
- evidence: Para ahorrar tokens de Claude se pasaron 6 miembros a Kiro; los intentos en curso (Heracles, Orin) siguieron en Claude y guild_decide retry respondió «task is Running». Propuesta: «restart» de un intento en curso con el harness/modelo nuevo, conservando el worktree.


## FB-055 · − Una quest Completed no acepta tareas nuevas

- kind: fails · cause: guild · status: open · project: harness-directive · by orchestrator · 2026-10-23T18:29:04.907Z
- evidence: Tras aceptar la especificación r3-spec la quest quedó Completed; las 4 tareas de implementación creadas después quedaron sin arrancar («quest is Completed») y hubo que crear otra quest. Propuesta: reabrir automáticamente una quest al agregarle tareas, o un comando reopen.

