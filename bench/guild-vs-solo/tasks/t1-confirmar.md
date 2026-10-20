Tamaño: chica. Origen: docs/INVESTIGACION.md, recomendación 10 (Frostpunk mobile: «perdí 100 de oro de un toque»).

Implementa en HARNESS: Directive la confirmación de gastos grandes y el cierre fácil de las ventanas:

1. Toda intervención que cueste 15 de Cómputo o más (hoy: zonificar 15 y reforzar 20) pide un segundo toque. El primer toque no gasta nada: el botón cambia a «¿CONFIRMAR? −N CÓMPUTO» en ámbar (Hud.AMBER). Un segundo toque sobre el mismo botón antes de 3 s ejecuta la intervención. Pasados 3 s, o si se toca otra cosa, el botón vuelve a su estado normal sin gastar.
2. Las intervenciones de menos de 15 (prioridad 5, despacho 8) siguen siendo de un toque.
3. En el modo aprendizaje, cuando una guía señala justamente ese botón, la intervención no pide confirmación: la guía ya está explicando qué tocar.
4. La ventana de evaluación de fase de la Directiva se cierra tocando en cualquier parte de la ventana, no solo en su botón, y hace lo mismo que el botón.
5. Pruebas (extiende tools/test_ui.gd o agrega una prueba nueva tools/test_<nombre>.gd que salga con código 1 si falla): prueba que un toque no gasta, que el segundo toque gasta exactamente el costo, que a los 3 s se cancela y que la evaluación se cierra tocando fuera del botón.

No cambies el balance ni los costos. No hagas commit.
