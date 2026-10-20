Tamaño: mediana. Origen: pregunta Q-004 del diseño vivo y docs/INVESTIGACION.md, recomendación 5 (Frostpunk: molesta que el final juzgue al jugador).

Implementa en HARNESS: Directive el registro sin sermón: el final de la Directiva muestra hechos, no una etiqueta moral.

1. DirectiveRun (scripts/sim/directive.gd) lleva una bitácora de hechos por fase: qué meta de qué facción se cumplió o falló, con su valor final (por ejemplo «Hospital con agua ≥ 80%: 64% del tiempo»), y qué costó o qué dio: Cómputo ganado o perdido, Orden perdido, manzana dañada (con sus coordenadas).
2. La bitácora entra en el guardado automático (scripts/save_game.gd): una partida restaurada a mitad de la Directiva conserva las fases anteriores.
3. Al terminar la Directiva, la pantalla final muestra por facción: metas cumplidas y fallidas, y la lista de hechos de cada fase. Termina con una sola línea neutra que diga a quién obedeciste más (por ejemplo «Obedeciste más a la Corporación: 3 de 3 metas»), o «No obedeciste del todo a nadie» si ninguna facción tiene saldo positivo.
4. Las palabras «Pragmática», «Benevolente», «Autoritaria», «Independiente» y la palabra «Ruta» desaparecen de la interfaz (también de la pestaña de registro, que hoy dice «Ruta actual: …»). DirectiveRun.route() puede seguir existiendo para uso interno, pero la interfaz no la muestra.
5. Pruebas: una prueba sin ventana nueva (por ejemplo tools/test_directive.gd, que salga con código 1 si falla) que juegue o simule las 3 fases con un resultado conocido y compruebe la bitácora, la línea final y que la bitácora sobrevive a guardar y restaurar.

No cambies el balance ni las metas. No hagas commit.
