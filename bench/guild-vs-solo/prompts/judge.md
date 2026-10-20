Eres un revisor senior de Godot (GDScript) y de diseño de juegos. Tres implementaciones independientes (P, Q y R) de la misma tarea, hechas sobre el mismo commit de HARNESS: Directive, están en {DIR}/P.diff, {DIR}/Q.diff y {DIR}/R.diff. El código base está en {REPO}; su diseño vivo, en {REPO}/game-design/ (empieza por overview.md). Léelo todo lo que necesites; no edites nada.

Resultado del verificador (pruebas de Godot sin ventana del proyecto más las pruebas nuevas de cada diff):
{VERIFY}

La tarea:

{TASK}

Evalúa cada implementación de 0 a 5 en:
- correctness: la lógica hace lo pedido y no tiene bugs (busca casos borde de verdad);
- spec_coverage: cubre todos los puntos de la tarea;
- test_quality: las pruebas prueban lo que dicen y fallarían si la funcionalidad se rompe;
- simplicity: sin complejidad innecesaria;
- fit_with_codebase: sigue las convenciones y la arquitectura existentes (simulación separada de la interfaz, etapas como datos, etc.);
- design_alignment: respeta el diseño vivo y no inventa lo que el diseño deja abierto;
- ux: lo que ve y toca el jugador funciona y se entiende en una pantalla vertical.

Lista los bugs concretos con archivo y línea. Ordénalas de mejor a peor y di por qué. Responde con un único bloque ```json y nada después:
{"P": {"correctness": 0, "spec_coverage": 0, "test_quality": 0, "simplicity": 0, "fit_with_codebase": 0, "design_alignment": 0, "ux": 0, "bugs": ["…"]}, "Q": {…}, "R": {…}, "ranking": ["P", "Q", "R"], "reason": "…"}
