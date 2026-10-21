# 2026-10-21 · harness-directive

Quest `01m5fbyjzsfepe6es9jtcqvq28` («Simulación Refuge 01 v0.1»). Miembros: Orin,
Lira, Kai, Teo (Kiro), Vesna (Claude). Orquestador: Claude Code (Opus 5.5).

### F-HD-01 · No se puede editar una tarea pendiente
- tipo: − · causa: guild · estado: abierta
- evidencia: para cambiar instrucciones o dependencias tuve que cancelar y recrear tareas seis veces (runner, integration, review ×4, actions, drones, econfix). La quest acumuló 32 tareas, 16 canceladas.
- costo: ruido en la pestaña Quests, ids con sufijos (`review7`), riesgo de dejar una dependencia colgada de una tarea cancelada.
- propuesta: `guild task set <id> [--instructions|--accept|--depends]` para tareas `pending`/`ready`; prueba: editar y que el scheduler respete las dependencias nuevas.

### F-HD-02 · `task new` falla sin `--strategy`
- tipo: − · causa: guild · estado: abierta
- evidencia: `guild task new … ` sin `--strategy` responde `invalid type: null, expected struct ExecutionStrategy`.
- costo: un intento perdido del orquestador.
- propuesta: estrategia `single` / selección `user` por defecto, como en `guild_start_quest`.

### F-HD-03 · Un permiso duplicado sin responder mata el intento
- tipo: − · causa: guild + harness · estado: propuesta (rama fix/guild-permissions: (a) y (b); (c) pendiente)
- evidencia: intento `01m5fbykcz5yx1exjwvwprpzvz` (core): la misma verificación pidió permiso dos veces; respondí la primera, la segunda quedó en espera 20 min y el intento murió por el límite de 30 min. El trabajo (≈900 líneas) se perdió: el reintento partió de cero.
- costo: ≈30 min y un intento.
- propuesta: (a) no contar el tiempo en `waiting_input` contra el límite del intento; (b) avisar en `guild_wait` cuántos minutos lleva esperando un permiso; (c) que un reintento por timeout parta del worktree del intento anterior.

### F-HD-04 · Cada lectura de solo lectura pide permiso
- tipo: − · causa: harness (Kiro) · estado: propuesta (rama fix/guild-permissions)
- evidencia: ≈150 permisos respondidos en la sesión; la mayoría `cat`, `grep`, `sed -n`, `git status`, `godot --headless … tools/test_*.gd`. Cada uno espera 10–35 s al orquestador.
- costo: horas de espera acumuladas; el orquestador no puede hacer otra cosa.
- propuesta: una lista de comandos seguros por proyecto en `.guild/guild.toml` (`[permissions] allow = ["godot --headless --path . -s tools/*", "git status", …]`) que la guild responda sola; lo demás sigue llegando al orquestador. Ojo: «always-accept» desde el orquestador fue bloqueado por el clasificador de Claude Code (Create Unsafe Agents); la lista tiene que venir del usuario, no del orquestador.

### F-HD-05 · El reintento no parte del intento anterior
- tipo: − · causa: guild · estado: abierta
- evidencia: al rechazar con instrucciones («parte de tu código actual»), el nuevo intento sale de la base de la quest; Orin encontró solo la rama del intento anterior (`guild/jtcqvq28/-economy/6grc4fnf`) con `git diff --cached`.
- costo: riesgo de rehacer todo y pasarse de los 30 min.
- propuesta: `guild_decide reject` con `--from-candidate` que siembre el worktree con el diff rechazado; o decir en el prompt del reintento el nombre de la rama anterior.

### F-HD-06 · Los miembros no ven cambios de diseño posteriores al inicio de la quest
- tipo: − · causa: guild · estado: abierta
- evidencia: cambié `docs/SIM_SPEC.md` en `main` tres veces durante la quest; los worktrees parten de la base de la quest. Tuve que copiar los cambios en cada tarea nueva («CAMBIOS POSTERIORES A LA ESPECIFICACIÓN…»), y los textos crecieron.
- costo: instrucciones de 8–10 k caracteres, riesgo de contradicciones.
- propuesta: comando para llevar un commit de `main` a la rama de la quest (`guild quest sync <quest>`), o que el prompt incluya el diff de docs/ entre la base y `main`.

### F-HD-07 · El límite de 30 min no alcanza para tareas de integración
- tipo: − · causa: guild + orquestador · estado: abierta
- evidencia: `integration5` (correr 84 partidas, calibrar, escribir informe) llegó a 30 min sin empezar el informe; la cancelé. Lo partí en `calib` + `report`.
- costo: un intento entero.
- propuesta: (orquestador) no juntar análisis largo y redacción en una tarea; (guild) límite por tarea (`--timeout`) y aviso a los 20 min en `guild_wait`.

### F-HD-08 · Errores de la especificación disfrazados de errores de los agentes
- tipo: − · causa: orquestador · estado: abierta
- evidencia: tres rechazos y dos tareas de arreglo vinieron de mi `SIM_SPEC.md` (dinero que no circulaba, subsidio al agua cobrado dos veces, escasez contra stock, arriendo a quien no tiene vivienda, sectores sin caja). Los agentes los encontraron y los preguntaron en vez de inventar.
- costo: ≈5 tareas.
- propuesta: antes de lanzar la primera quest de un modelo económico, una tarea de revisión de la especificación (solo lectura) con una comprobación de conservación (dinero, agua, materiales) escrita a mano; hacerlo parte del brief del orquestador para sistemas con flujos.

### F-HD-09 · + La revisión temprana en paralelo
- tipo: + · causa: orquestador · estado: abierta
- evidencia: Teo (reviewer, solo lectura) revisó lo integrado mientras Orin calibraba: comprobación de conservación del dinero por términos sobre 3 650 días, 10 hallazgos verificados con caso y corrección. Evitó calibrar sobre errores de contabilidad.
- propuesta: que el brief recomiende un reviewer de solo lectura en paralelo apenas haya algo integrado, no solo al final.

### F-HD-10 · + Pruebas que fallan antes del arreglo y pruebas de mutación
- tipo: + · causa: rol (gameplay-engineer) · estado: abierta
- evidencia: todos los miembros comprobaron que sus pruebas fallan con el código anterior y rompieron reglas a propósito para ver la prueba fallar. Así se encontró el bug de `PackedFloat64Array` en `run_refuge.gd` (las series quedaban vacías y el informe habría dicho «nada oscila»).
- propuesta: mantener esto en el prompt del rol; es lo que más valor dio.

### F-HD-11 · + Los miembros preguntan en vez de inventar
- tipo: + · causa: rol + brief · estado: abierta
- evidencia: cada resumen trajo «Preguntas» con la decisión que tomaron por defecto (Orin: circulación del dinero, freno sin tubería; Lira: sentido de las flechas, nivel de TRABAJO).
- propuesta: mantener.

### F-HD-12 · Comandos que dejan el worktree en un estado inconsistente
- tipo: − · causa: rol · estado: abierta
- evidencia: Orin pidió restaurar `refuge_sim.gd` y `refuge_params.gd` antiguos para medir «antes», pero el comando solo devolvía `refuge_sim.gd` nuevo (el `refuge_params.gd` habría quedado viejo). Rechacé el permiso.
- propuesta: en el prompt del rol, para comparar con el código anterior usar `git stash` o `git worktree add` en un directorio temporal, no copiar archivos a mano.

### F-HD-13 · Archivos temporales fuera del worktree
- tipo: − · causa: rol + guild · estado: propuesta (rama fix/guild-permissions)
- evidencia: scripts y CSV en `/tmp` (`/tmp/econfix2`, `/tmp/refuge0` ≈ 500 MB); un `rm -rf` fue rechazado por la política y quedaron sin borrar.
- propuesta: que la guild dé a cada intento un `$GUILD_TMP` propio y lo borre al cerrar; el prompt del rol lo usa en vez de `/tmp`.

### F-HD-14 · Verificador de capturas inestable
- tipo: − · causa: proyecto · estado: abierta
- evidencia: `captures` falló dos veces por `Alarm clock` (60 s) al abrir la ventana, en tareas que no tocan nada visible.
- propuesta: (proyecto) subir el `alarm` o reintentar una vez; (guild) poder marcar una tarea como «sin cambios visibles» para no correr capturas.
