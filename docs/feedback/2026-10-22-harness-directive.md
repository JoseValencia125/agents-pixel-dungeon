# 2026-10-22 · harness-directive

Quest `01m5fbyjzsfepe6es9jtcqvq28` («Simulación Refuge 01 v0.1»), rondas 4 y
limpieza (DEC-075). Miembros: Tomás, Vesna (Claude), Orin, Ilse, Bruno (Kiro).
Orquestador: Claude Code (Opus 5.5). Ids de `guild feedback`: FB-033…FB-041.

### FB-033 · Hice yo un trabajo largo de documento en vez de delegarlo
- tipo: − · causa: orquestador · estado: propuesta
- evidencia: armé el doc de diseño (6 secciones + pestaña visual) dentro de la conversación; el usuario no pudo seguir hablando y pidió que se delegara.
- costo: la conversación con el diseñador quedó bloqueada mientras se escribía el doc.
- propuesta: el brief dice que el delegado manda a un subagente o a un miembro todo trabajo de más de un par de llamadas (documentos, lecturas largas, merges, esperas) y queda libre para conversar.

### FB-034 · No hay forma de reabrir una pregunta resuelta por error
- tipo: − · causa: guild · estado: propuesta
- evidencia: Q-048 se resolvió con la decisión de Q-047; tuve que crear Q-056 como copia.
- costo: un id nuevo, la respuesta del usuario y la historia quedan en dos preguntas.
- propuesta: `guild question reopen <Q-id>` (y `guild_question` con `reopen`), que deshaga la decisión enlazada si fue solo para esa pregunta.

### FB-035 · Cancelar una tarea deja su entrega «submitted» y el miembro queda ocupado
- tipo: − · causa: guild · estado: propuesta
- evidencia: `jtcqvq28-e2b` cancelada con el intento de Tomás en `submitted`; Tomás figuraba `busy` y la tarea nueva `jtcqvq28-cleanup` quedó en `Ready` sin arrancar. `guild_decide reject` sobre la tarea cancelada respondió `conflict: task cannot go from Cancelled to Ready`, pero igual liberó al miembro.
- costo: una tarea parada sin aviso hasta que lo noté a mano.
- propuesta: al cancelar, cerrar los intentos `submitted`/`running` de la tarea y liberar al miembro; `guild_status` marca a un miembro `busy` sin tarea viva.

### FB-036 · Volví a delegar tarde: merge y pruebas a `main` hechos por mí
- tipo: − · causa: orquestador · estado: propuesta
- evidencia: después de FB-033, el usuario lo corrigió por segunda vez («Volviste a tomar una tarea sin delegarla») cuando hice el merge, el import y las pruebas en la conversación.
- costo: la misma de FB-033; el patrón se repite aunque la lección estaba anotada.
- propuesta: igual que FB-033, con el ejemplo explícito en el brief: «un merge con verificación también se delega, aunque parezca corto».

### FB-037 · Retuve el merge a `main` esperando el fin de la ronda sin decírselo al usuario
- tipo: − · causa: orquestador · estado: propuesta
- evidencia: la quest tenía 25 commits verificados fuera de `main`; otros agentes que analizaban el repo veían solo el prototipo anterior. El usuario preguntó por qué tardaba «un simple push y merge».
- costo: confusión de otros agentes y del usuario sobre el estado del repo.
- propuesta: el brief recomienda integrar a la rama objetivo cada tanda verificada (o `--auto-integrate` por tarea), no al final de la quest, y decir al usuario cuándo se integra.

### FB-038 · Al integrar una rama con `class_name` nuevos, parse_check falla hasta importar
- tipo: − · causa: guild · estado: propuesta
- evidencia: merge de `guild/quest/jtcqvq28` a `main`: `PARSE FAIL` en 12 scripts («RefugeCities not declared»). Tras `godot --headless --import` dos veces: `PARSE OK` y `run_tests OK`.
- costo: un falso rojo justo antes de subir.
- propuesta: que la integración corra los pasos de preparación del proyecto (`[verify] setup` en `guild.toml`, aquí el import) antes de los verificadores, igual que en los worktrees.

### FB-039 · Un revisor marcó como hallazgo una regla que el diseñador cambió después de que empezó su tarea
- tipo: − · causa: guild · estado: propuesta
- evidencia: Vesna (`r4review`) contó «arriendo primero» como contradicción con DEC-054, pero DEC-059 lo reemplazó durante su tarea. FB-006 (`quest sync`) está aplicada, pero no llega a una tarea en curso; hubo que mandarlo con una nota.
- costo: un hallazgo falso y una nota a mano.
- propuesta: cuando se registra una decisión que reemplaza a otra, la guild agrega una nota automática a las tareas en curso de la quest que citan la decisión reemplazada.

### FB-040 · El orquestador dejó vencer un permiso inofensivo
- tipo: − · causa: orquestador · estado: propuesta
- evidencia: Vesna esperó más de 2 min por un `sh` sobre scripts de su propio directorio de borradores (mutaciones); el orquestador escaló con «timed out» en vez de decidir. Parecido a FB-015.
- costo: el revisor parado hasta que lo vi.
- propuesta: la política aprueba sola los comandos que solo tocan `$GUILD_TMP` del intento y el worktree; el orquestador recibe solo lo que sale de ahí.

### FB-041 · `guild_wait` sin `questId` falla tarde y en silencio
- tipo: − · causa: guild · estado: propuesta
- evidencia: lanzado en segundo plano sin `questId`, salió con `missing questId` recién al terminar, y perdí la espera.
- costo: una espera perdida sin saberlo.
- propuesta: sin `questId`, esperar a todas las quests del proyecto; si no, fallar al instante antes de esperar.
