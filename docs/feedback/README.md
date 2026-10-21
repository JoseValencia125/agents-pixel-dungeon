# Feedback del orquestador

Lo que va sirviendo y lo que no al trabajar con la guild y sus miembros, escrito
por el orquestador (o por el usuario) mientras trabaja. Sirve para mejorar los
prompts de los roles, el brief del orquestador, las skills y la propia guild.

Complementa, no reemplaza:
- `docs/GUILD-NOTES.md`: qué se construyó en cada hito.
- `AttemptRecord` (`guild records`): métricas por intento. Cada entrada de
  aquí cita IDs de intentos o tareas para poder cruzarla con esas filas.

## Cómo se escribe y se lee

```
guild feedback new --project <p> --kind works|fails --cause role|brief|orchestrator|guild|harness|project \
                   --title "…" --evidence "…" [--cost "…"] [--proposal "…"] [--ref <quest|task|attempt>]…
guild feedback list --status open            # una línea por entrada
guild feedback export --status open          # Markdown, para el prompt de un agente
guild feedback set FB-001 --status proposed --proposal "…"
guild feedback set FB-001 --status applied --commit <sha> --note "cómo se comprobó"
guild feedback set FB-002 --status discarded --note "por qué no"
```

Las entradas viven en la base de la guild (tabla `feedback`, en el snapshot) y
se numeran `FB-001`, `FB-002`… para toda la guild. Un agente que trabaje en este
repositorio empieza por `guild feedback export --status open`, propone cambios
(`proposed`) y marca `applied` con el commit cuando lo aplica.

Los archivos Markdown de esta carpeta son el registro anterior al comando; sus
entradas se cargan con `guild feedback new` cuando el servicio corre la versión
que lo trae.

## Reglas

1. **Una entrada = una observación con evidencia.** Qué pasó, dónde (proyecto,
   quest, tarea, intento) y cuántas veces. Sin evidencia no hay entrada.
2. **Clasificar la causa**, porque no todo es culpa del agente:
   - `rol`: el prompt de un rol produjo (o evitó) el problema.
   - `brief`: el brief del orquestador (`guild orchestrate`) guió mal o bien.
   - `orquestador`: error mío al escribir la tarea o la especificación.
   - `guild`: la herramienta (CLI, servicio, scheduler, límites).
   - `harness`: Kiro, Codex, Claude (permisos, sesiones, tiempos).
   - `proyecto`: el repositorio del juego (pruebas, verificadores).
3. **Separar observación y cambio.** Una entrada puede proponer un cambio, pero
   el cambio se aplica después, entre quests, y con una prueba que muestre que
   mejora (repetir una tarea con el harness `mock` o con un miembro real).
   Nunca se cambia la guild mientras corre una quest.
4. **Lo que funcionó también se anota** (`+`), para no perderlo al reescribir.
5. **Estado**: `abierta` → `propuesta` (hay cambio redactado) → `aplicada`
   (con commit) o `descartada` (con motivo).

## Formato

Un archivo por sesión: `AAAA-MM-DD-<proyecto>.md`. Cada entrada:

```
### F-<proyecto>-NN · <título corto>
- tipo: + | − · causa: rol | brief | orquestador | guild | harness | proyecto · estado: abierta
- evidencia: <quest/tarea/intento, qué pasó, cuántas veces>
- costo: <tiempo, intentos perdidos, riesgo>
- propuesta: <cambio concreto y cómo probarlo>
```

## Cuándo se procesa

Al cerrar cada quest, el orquestador repasa las entradas `open` de esa sesión,
redacta propuestas para las que se repitieron o costaron caro, y se las muestra
al usuario. El brief del orquestador (`guild orchestrate`) ya le dice cómo
escribirlas. Pendiente: una pestaña en el dungeon.
