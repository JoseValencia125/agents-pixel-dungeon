# 2026-10-23 · harness-directive

Quests `01m5k1r65sfyq6naavpkkesx6w` («Refuge 02»: cinco tareas de simulación en
paralelo, calibración, estrategias y revisión con mutaciones) y
`01m5kc2nhegk4k744d6r9gkj2r` («Refuge visual 02», seis artistas). Miembros:
Marga, Tomás, Orin, Brecha, Vesna (Claude); Lira, Bruno, Kai, Alba, Ciro, Dana,
Eva, Fede, Gala (Kiro). Orquestador: Claude Code (Opus 5.5). Ids de
`guild feedback`: FB-043…FB-047, y FB-035 que se repitió. Casi todo el roce
vino de integrar trabajo paralelo.

### FB-035 (se repitió) · Cancelar una tarea deja su entrega «submitted» y el miembro queda ocupado
- tipo: − · causa: guild · estado: propuesta
- evidencia: volvió a pasar en la quest siguiente: Orin (`jtcqvq28-e2c`) y Lira (`jtcqvq28-e2a`) seguían `busy` por entregas `submitted` de tareas canceladas, y `r2-protocols` y `r2-upgrades` quedaron en `Ready` sin arrancar. El mismo truco (`reject` → error de conflicto, pero libera) los soltó.
- costo: dos tareas paradas sin aviso al inicio de la noche.
- propuesta: la de FB-035; además, al iniciar una quest, avisar si un miembro asignado está ocupado por una tarea cancelada.

### FB-043 · El orquestador escala por falta de evidencia que está en la respuesta completa del miembro
- tipo: − · causa: guild · estado: propuesta
- evidencia: `r2-r4fixes` (Lira): el orquestador pidió la tabla mutación → prueba y la comparación de partidas; ambas estaban en la respuesta entera (`guild_task full`), no en el resumen cortado que ve. Pasó también con casi todas las entregas visuales: «el diff está cortado», sin mirar capturas ni documentos.
- costo: cada entrega buena pasó por mí; una decena de escalamientos en la noche.
- propuesta: el orquestador lee la respuesta completa y los archivos del candidato (documentos, imágenes) antes de escalar por falta de evidencia; variante de FB-016/FB-025.

### FB-044 · Las tareas paralelas chocan al integrar y la guild no ofrece traer la rama de la quest
- tipo: − · causa: guild · estado: propuesta
- evidencia: `r2-offline` (Kai) quedó `Blocked` por conflicto de squash con `r2-r4fixes`, integrada antes (`refuge_params.gd`, `refuge_sim.gd`). Lo mismo pasó con `r2-education` y `v2-efectos`. Mandarlo de vuelta gastó el último intento de Kai.
- costo: tres integraciones bloqueadas y un intento perdido.
- propuesta: antes del squash, la guild intenta traer la rama de la quest al candidato; si hay conflicto, lo devuelve al miembro sin contar un intento, con la lista de archivos en conflicto.

### FB-045 · No se puede marcar Completed una tarea integrada a mano tras un conflicto
- tipo: − · causa: guild · estado: propuesta
- evidencia: `r2-education` quedó `Blocked`; el integrador la juntó a mano en `guild/quest/pkkesx6w` (`a9d0df6`, `run_tests OK`). `accept --attempt --override` responde «attempt is Passed, not submitted». Hubo que cancelarla, y `r2-calib`, que dependía de ella, quedó bloqueada («dependency failed»); se recrearon tres tareas.
- costo: tres tareas canceladas y recreadas a mano.
- propuesta: `accept <task> --attempt <id> --manual <sha>` marca la tarea `Completed` con el commit de la integración a mano y libera a sus dependientes.

### FB-046 · Pedí a los miembros traer la rama de la quest, pero el orquestador de la guild bloquea `git merge`
- tipo: − · causa: orquestador · estado: propuesta
- evidencia: Lira (`r2-upgrades`) intentó `git merge guild/quest/pkkesx6w` cuatro veces y el orquestador (Oro) lo rechazó cada vez; gastó el turno preguntando. Lo correcto era integrar yo a mano desde el principio, con un agente integrador.
- costo: un turno de Lira y notas contradictorias a cinco tareas.
- propuesta: el brief dice que los miembros no hacen merge; con tareas paralelas sobre archivos compartidos, el orquestador planea un integrador desde el inicio.

### FB-047 · Un miembro entrega mientras sus procesos siguen en segundo plano y quema sus intentos
- tipo: − · causa: guild · estado: propuesta
- evidencia: Vesna (`r2-review2`) lanzó las mutaciones en segundo plano, terminó el turno y entregó `REFUGE_REVIEW_05` con marcadores `__MUTTABLE__` sin llenar; el orquestador falló los 3 intentos. Con la instrucción explícita de correr en primer plano (`r2-review3`) salió bien.
- costo: tres intentos de una revisión larga (opus) perdidos.
- propuesta: no aceptar una entrega con procesos hijos vivos del miembro (o esperar a que terminen), y avisarle al miembro antes de cerrar el turno.

### FB-048 · La guild permite dos miembros con el mismo nombre
- tipo: − · causa: guild · estado: propuesta
- evidencia: había dos «Lira» (`lira` y `tools-eng`) y dos «Orin» (`orin` en Kiro y `sim-eng` en Claude). En `guild_status` y en las escalaciones no se sabía cuál era cuál. El usuario pidió nombres únicos de héroes griegos y romanos, y hubo que renombrar a mano (`lira` → Teseo, `orin` → Perseo).
- costo: confusión al asignar tareas y al leer escalaciones durante toda la noche.
- propuesta: `member add` y `member set --name` rechazan un nombre que ya usa otro miembro; si no se da `--name`, la guild propone uno único de una lista temática (por ejemplo, héroes y semidioses griegos y romanos) que el proyecto puede configurar.
