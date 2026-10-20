Tamaño: grande. Origen: sistema «Modo aprendizaje» del diseño vivo (etapas 4–7 en preparación; pregunta Q-003).

Implementa en HARNESS: Directive la etapa 4 del modo aprendizaje, «Producción». Enseña una sola idea nueva: la industria produce solo si tiene energía, y cada megavatio que va a las fábricas es uno que no va a otro lado.

1. La etapa 4 va después de «Agua» en scripts/sim/stages.gd y se describe solo con datos, como las otras tres. Se suma el sector Noreste (el de las fábricas y la planta de energía): jurisdicción NO, SE y NE. Agua sí, Orden no. Pestañas: CONSTRUIR, GESTIONAR y la de simulación. Recursos visibles: ENERGÍA, AGUA, CÓMPUTO y PRODUCCIÓN.
2. Aparece la cuota de producción. La Corporación fija la cuota en 100%, y el panel la muestra como meta sin reloj. Metas de la etapa:
   a. «Zonifica una fábrica» (en una manzana libre).
   b. «Mantén la producción ≥ 100% durante 30 s seguidos».
3. Evento guionado: a los 8 s de la etapa, la demanda del Noreste sube 25% (como los «spike» de las otras etapas). Esto empuja la subestación NE hacia el disparo mientras el jugador intenta cumplir la cuota.
4. Guías, en orden, con el mismo sistema de las otras etapas (when, target, pause, until, tap, duration):
   - presentar el sector Noreste y la cuota;
   - señalar la manzana libre y pedir zonificar una fábrica;
   - cuando la subestación NE se caliente, señalar su alerta;
   - en la traza, explicar que las fábricas son el mayor consumidor del cuadrante;
   - pedir una de dos salidas: reforzar la subestación NE, o bajar las fábricas a NORMAL y subir la vivienda a ALTA. Cualquiera de las dos completa la guía.
   Al cerrar, «learned» explica que la producción depende de la energía y que priorizar la industria deja a otros sin servicio.
5. Si para medir «≥ 100% durante 30 s seguidos» hace falta un contador o una condición nueva en StageRun, agrégala de forma general (que la puedan usar otras etapas), no como caso especial de la etapa 4.
6. Al terminar la etapa 4 se pasa a la Directiva completa, como hoy al terminar la 3.
7. Pruebas: tools/test_stages.gd y tools/test_learn.gd tienen que completar las 4 etapas (test_learn tocando lo que señala cada guía, como hoy). Si una etapa no se puede completar siguiendo sus guías, es un error de la etapa, no de la prueba.

No cambies el balance de las etapas 1–3 ni de la Directiva. Lo que el diseño no decide (por ejemplo, textos exactos de las guías) escríbelo con el tono de las etapas existentes y nómbralo en tu resumen. No hagas commit.
