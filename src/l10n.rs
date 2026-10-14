//! UI text in Spanish (the source language), English, French, Italian and
//! Portuguese. The language follows the system locale (macOS: Language &
//! Region; Linux: LANG / LC_MESSAGES); anything else falls back to English.
//! Placeholders are `{}`, filled in order by `trf`.

use std::collections::HashMap;
use std::sync::LazyLock;

pub const LANGUAGES: [&str; 5] = ["es", "en", "fr", "it", "pt"];

/// Spanish source text → [en, fr, it, pt].
pub static TABLE: LazyLock<HashMap<&'static str, [&'static str; 4]>> = LazyLock::new(|| {
    let rows: &[(&str, [&str; 4])] = &[
        // The guild: quests, tasks and attempts
        ("Interrumpir", ["Interrupt", "Interrompre", "Interrompi", "Interromper"]),
        ("Crear", ["Create", "Créer", "Crea", "Criar"]),
        ("modelo", ["model", "modèle", "modello", "modelo"]),
        ("Tablón de misiones", ["Quest board", "Tableau des quêtes", "Bacheca delle missioni", "Quadro de missões"]),
        ("Servicio de la guild apagado", ["Guild service off", "Service de la guilde arrêté", "Servizio della gilda spento", "Serviço da guilda desligado"]),
        ("Servicio de la guild conectado", ["Guild service connected", "Service de la guilde connecté", "Servizio della gilda collegato", "Serviço da guilda ligado"]),
        ("Servicio de la guild arrancado", ["Guild service started", "Service de la guilde démarré", "Servizio della gilda avviato", "Serviço da guilda iniciado"]),
        ("Arrancar el servicio", ["Start the service", "Démarrer le service", "Avvia il servizio", "Iniciar o serviço"]),
        ("No encuentro agents-pixel-dungeon-guild junto a la app", ["Cannot find agents-pixel-dungeon-guild next to the app", "Impossible de trouver agents-pixel-dungeon-guild à côté de l'app", "Non trovo agents-pixel-dungeon-guild accanto all'app", "Não encontro agents-pixel-dungeon-guild ao lado do app"]),
        ("Sin misiones todavía", ["No quests yet", "Pas encore de quêtes", "Ancora nessuna missione", "Ainda sem missões"]),
        ("Guild: {}", ["Guild: {}", "Guilde : {}", "Gilda: {}", "Guilda: {}"]),
        ("Misión iniciada: {}", ["Quest started: {}", "Quête lancée : {}", "Missione avviata: {}", "Missão iniciada: {}"]),
        ("Misión completada: {}", ["Quest completed: {}", "Quête terminée : {}", "Missione completata: {}", "Missão concluída: {}"]),
        ("{} toma {}", ["{} takes {}", "{} prend {}", "{} prende {}", "{} assume {}"]),
        ("{} pide permiso: {}", ["{} asks permission: {}", "{} demande une permission : {}", "{} chiede il permesso: {}", "{} pede permissão: {}"]),
        ("{} entregó un candidato para {}", ["{} submitted a candidate for {}", "{} a livré un candidat pour {}", "{} ha consegnato un candidato per {}", "{} entregou um candidato para {}"]),
        ("{} falló en {}: {}", ["{} failed on {}: {}", "{} a échoué sur {} : {}", "{} ha fallito su {}: {}", "{} falhou em {}: {}"]),
        ("Verificador {}: {}", ["Verifier {}: {}", "Vérificateur {} : {}", "Verificatore {}: {}", "Verificador {}: {}"]),
        ("Integrado: {}", ["Integrated: {}", "Intégré : {}", "Integrato: {}", "Integrado: {}"]),
        ("Integración bloqueada: {}", ["Integration blocked: {}", "Intégration bloquée : {}", "Integrazione bloccata: {}", "Integração bloqueada: {}"]),
        ("Tarea bloqueada: {} · {}", ["Task blocked: {} · {}", "Tâche bloquée : {} · {}", "Compito bloccato: {} · {}", "Tarefa bloqueada: {} · {}"]),
        ("{} · preparando el taller", ["{} · setting up the workshop", "{} · prépare l'atelier", "{} · prepara la bottega", "{} · preparando a oficina"]),
        ("{} · pide permiso: {}", ["{} · asks permission: {}", "{} · demande une permission : {}", "{} · chiede il permesso: {}", "{} · pede permissão: {}"]),
        ("en cola", ["queued", "en file", "in coda", "na fila"]),
        ("preparando el taller", ["setting up the workshop", "prépare l'atelier", "prepara la bottega", "preparando a oficina"]),
        ("trabajando", ["working", "au travail", "al lavoro", "trabalhando"]),
        ("esperando permiso", ["waiting for permission", "attend une permission", "attende il permesso", "esperando permissão"]),
        ("candidato entregado", ["candidate submitted", "candidat livré", "candidato consegnato", "candidato entregue"]),
        ("verificando", ["verifying", "vérification", "verifica", "verificando"]),
        ("aceptado", ["accepted", "accepté", "accettato", "aceito"]),
        ("fallido", ["failed", "échoué", "fallito", "falhou"]),
        ("cancelado", ["cancelled", "annulé", "annullato", "cancelado"]),
        ("perdido", ["lost", "perdu", "perso", "perdido"]),
        ("tiempo agotado", ["timed out", "délai dépassé", "tempo scaduto", "tempo esgotado"]),
        ("{} · falló: {}", ["{} · failed: {}", "{} · a échoué : {}", "{} · fallito: {}", "{} · falhou: {}"]),
        ("{} · verificando · {}", ["{} · verifying · {}", "{} · vérifie · {}", "{} · verifica · {}", "{} · verificando · {}"]),
        ("{} · integrando · {}", ["{} · integrating · {}", "{} · intègre · {}", "{} · integra · {}", "{} · integrando · {}"]),
        ("{} · candidato listo · {}", ["{} · candidate ready · {}", "{} · candidat prêt · {}", "{} · candidato pronto · {}", "{} · candidato pronto · {}"]),
        ("{} · bloqueado · {}", ["{} · blocked · {}", "{} · bloqué · {}", "{} · bloccato · {}", "{} · bloqueado · {}"]),
        ("{} · libre · {}", ["{} · free · {}", "{} · libre · {}", "{} · libero · {}", "{} · livre · {}"]),
        ("Libre · {}", ["Free · {}", "Libre · {}", "Libero · {}", "Livre · {}"]),
        ("modelo por defecto", ["default model", "modèle par défaut", "modello predefinito", "modelo padrão"]),
        ("{} archivos · +{} −{}", ["{} files · +{} −{}", "{} fichiers · +{} −{}", "{} file · +{} −{}", "{} arquivos · +{} −{}"]),
        ("Pide permiso: {}", ["Asks permission: {}", "Demande une permission : {}", "Chiede il permesso: {}", "Pede permissão: {}"]),
        ("Aceptar e integrar", ["Accept and integrate", "Accepter et intégrer", "Accetta e integra", "Aceitar e integrar"]),
        ("Integrar", ["Integrate", "Intégrer", "Integra", "Integrar"]),
        ("Reintentar", ["Retry", "Réessayer", "Riprova", "Tentar de novo"]),
        ("Cancelar intento", ["Cancel attempt", "Annuler la tentative", "Annulla il tentativo", "Cancelar tentativa"]),
        ("Candidato: {} archivos", ["Candidate: {} files", "Candidat : {} fichiers", "Candidato: {} file", "Candidato: {} arquivos"]),
        ("Integrado en la rama de la misión", ["Integrated into the quest's branch", "Intégré dans la branche de la quête", "Integrato nel ramo della missione", "Integrado no ramo da missão"]),
        ("Iniciar", ["Start", "Lancer", "Avvia", "Iniciar"]),
        ("Pausar", ["Pause", "Pause", "Pausa", "Pausar"]),
        ("Reanudar", ["Resume", "Reprendre", "Riprendi", "Retomar"]),
        ("           espera a {}", ["           waits for {}", "           attend {}", "           attende {}", "           espera {}"]),
        ("Efectos de estado", ["Status effects", "Effets d'état", "Effetti di stato", "Efeitos de estado"]),
        ("Contrato de datos", ["Data contract", "Contrat de données", "Contratto dati", "Contrato de dados"]),
        ("Lógica de combate", ["Combat logic", "Logique de combat", "Logica di combattimento", "Lógica de combate"]),
        ("Interfaz", ["Interface", "Interface", "Interfaccia", "Interface"]),
        ("Pruebas de aceptación", ["Acceptance tests", "Tests d'acceptation", "Test di accettazione", "Testes de aceitação"]),
        ("Integración y QA visual", ["Integration and visual QA", "Intégration et QA visuelle", "Integrazione e QA visiva", "Integração e QA visual"]),
        ("Añadiendo el tick de veneno al combate", ["Adding the poison tick to combat", "Ajoute le tick de poison au combat", "Aggiunge il tick di veleno al combattimento", "Adicionando o tick de veneno ao combate"]),
        ("Fixtures y pruebas del contrato listos", ["Fixtures and contract tests ready", "Fixtures et tests du contrat prêts", "Fixture e test del contratto pronti", "Fixtures e testes do contrato prontos"]),
        ("Leo el contrato de efectos y la entidad de combate.", ["Reading the effects contract and the combat entity.", "Je lis le contrat des effets et l'entité de combat.", "Leggo il contratto degli effetti e l'entità di combattimento.", "Leio o contrato de efeitos e a entidade de combate."]),
        ("Añado PoisonEffect con duración y daño por tick, sin duplicar ticks al reaplicar.", ["Adding PoisonEffect with duration and damage per tick, without duplicating ticks on reapply.", "J'ajoute PoisonEffect avec durée et dégâts par tick, sans doubler les ticks à la réapplication.", "Aggiungo PoisonEffect con durata e danno per tick, senza duplicare i tick alla riapplicazione.", "Adiciono PoisonEffect com duração e dano por tick, sem duplicar ticks ao reaplicar."]),
        ("La barra de estado muestra pilas y duración.", ["The status bar shows stacks and duration.", "La barre d'état montre les piles et la durée.", "La barra di stato mostra pile e durata.", "A barra de estado mostra pilhas e duração."]),
        ("Fixtures y pruebas del contrato listos. Las pruebas funcionales esperan a T2 y T3.", ["Fixtures and contract tests ready. The functional tests wait for T2 and T3.", "Fixtures et tests du contrat prêts. Les tests fonctionnels attendent T2 et T3.", "Fixture e test del contratto pronti. I test funzionali attendono T2 e T3.", "Fixtures e testes do contrato prontos. Os testes funcionais esperam por T2 e T3."]),
        // Room titles
        ("Ocultar terminal", ["Hide terminal", "Masquer le terminal", "Nascondi terminale", "Ocultar terminal"]),
        ("Terminal externa", ["External terminal", "Terminal externe", "Terminale esterno", "Terminal externo"]),
        ("Nueva sesión", ["New session", "Nouvelle session", "Nuova sessione", "Nova sessão"]),
        ("Sesión terminada", ["Session ended", "Session terminée", "Sessione terminata", "Sessão encerrada"]),
        ("Cerrar la ventana conserva la sesión. Usa exit para terminarla.", ["Closing the window keeps the session. Use exit to end it.", "Fermer la fenêtre conserve la session. Utilisez exit pour la terminer.", "Chiudere la finestra conserva la sessione. Usa exit per terminarla.", "Fechar a janela mantém a sessão. Use exit para encerrá-la."]),
        ("TRABAJANDO", ["WORKING", "AU TRAVAIL", "AL LAVORO", "TRABALHANDO"]),
        ("ATENCIÓN", ["ATTENTION", "ATTENTION", "ATTENZIONE", "ATENÇÃO"]),
        ("EN ESPERA", ["WAITING", "EN ATTENTE", "IN ATTESA", "EM ESPERA"]),
        ("LISTO", ["DONE", "TERMINÉ", "FATTO", "PRONTO"]),
        ("SIN ESTADO", ["NO STATUS", "SANS ÉTAT", "SENZA STATO", "SEM ESTADO"]),
        ("AGOTADO", ["EXHAUSTED", "ÉPUISÉ", "ESAUSTO", "ESGOTADO"]),
        ("Límite de sesión", ["Session limit", "Limite de session", "Limite di sessione", "Limite de sessão"]),
        ("Añadir a favoritas", ["Add to favourites", "Ajouter aux favoris", "Aggiungi ai preferiti", "Adicionar aos favoritos"]),
        ("Quitar de favoritas", ["Remove from favourites", "Retirer des favoris", "Rimuovi dai preferiti", "Remover dos favoritos"]),
        // HP viewer
        ("lun", ["Mon", "lun", "lun", "seg"]),
        ("mar", ["Tue", "mar", "mar", "ter"]),
        ("mié", ["Wed", "mer", "mer", "qua"]),
        ("jue", ["Thu", "jeu", "gio", "qui"]),
        ("vie", ["Fri", "ven", "ven", "sex"]),
        ("sáb", ["Sat", "sam", "sab", "sáb"]),
        ("dom", ["Sun", "dim", "dom", "dom"]),
        ("HP de la guild (uso restante)", ["Guild HP (usage left)", "PV de la guilde (usage restant)", "PV della gilda (uso rimanente)", "HP da guilda (uso restante)"]),
        ("{} créditos", ["{} credits", "{} crédits", "{} crediti", "{} créditos"]),
        ("En vivo (token)", ["Live (token)", "En direct (jeton)", "In tempo reale (token)", "Ao vivo (token)"]),
        ("Último /usage", ["Last /usage", "Dernier /usage", "Ultimo /usage", "Último /usage"]),
        ("Lo que Claude Code guardó la última vez que corriste /usage. No lee credenciales.", ["What Claude Code saved the last time you ran /usage. No credentials are read.", "Ce que Claude Code a gardé la dernière fois que tu as lancé /usage. Aucun identifiant n'est lu.", "Ciò che Claude Code ha salvato l'ultima volta che hai eseguito /usage. Nessuna credenziale viene letta.", "O que o Claude Code guardou da última vez que você rodou /usage. Nenhuma credencial é lida."]),
        ("Pregunta a Anthropic cada 2 min con el token de Claude Code (llavero de macOS o ~/.claude/.credentials.json).", ["Asks Anthropic every 2 min with Claude Code's token (macOS keychain or ~/.claude/.credentials.json).", "Interroge Anthropic toutes les 2 min avec le jeton de Claude Code (trousseau macOS ou ~/.claude/.credentials.json).", "Chiede ad Anthropic ogni 2 min con il token di Claude Code (portachiavi di macOS o ~/.claude/.credentials.json).", "Pergunta à Anthropic a cada 2 min com o token do Claude Code (chaves do macOS ou ~/.claude/.credentials.json)."]),
        ("Elegir plan…", ["Pick plan…", "Choisir l'offre…", "Scegli piano…", "Escolher plano…"]),
        ("sem", ["wk", "sem", "sett", "sem"]),
        ("mes", ["mo", "mois", "mese", "mês"]),
        ("{}: {}% usado", ["{}: {}% used", "{} : {} % utilisé", "{}: {}% usato", "{}: {}% usado"]),
        ("se reinicia en {}", ["resets in {}", "réinitialisation dans {}", "si azzera tra {}", "reinicia em {}"]),
        // Status labels
        ("Trabajando", ["Working", "Au travail", "Al lavoro", "Trabalhando"]),
        ("Necesita atención", ["Needs attention", "Demande ton attention", "Richiede attenzione", "Precisa de atenção"]),
        ("En espera", ["Waiting", "En attente", "In attesa", "Em espera"]),
        ("Listo", ["Done", "Terminé", "Fatto", "Pronto"]),
        ("Sin estado", ["No status", "Sans état", "Senza stato", "Sem estado"]),
        // Window, menu and status item
        ("Minimizar (ocultar)", ["Minimize (hide)", "Réduire (masquer)", "Riduci (nascondi)", "Minimizar (ocultar)"]),
        ("Cerrar", ["Close", "Fermer", "Chiudi", "Fechar"]),
        ("Arrastrar para mover", ["Drag to move", "Glisser pour déplacer", "Trascina per spostare", "Arraste para mover"]),
        ("Mostrar / ocultar", ["Show / hide", "Afficher / masquer", "Mostra / nascondi", "Mostrar / ocultar"]),
        ("Reposicionar en la esquina", ["Move back to the corner", "Replacer dans le coin", "Riposiziona nell'angolo", "Reposicionar no canto"]),
        ("Acerca de Agents Pixel Dungeon", ["About Agents Pixel Dungeon", "À propos de Agents Pixel Dungeon", "Informazioni su Agents Pixel Dungeon", "Sobre o Agents Pixel Dungeon"]),
        ("Salir", ["Quit", "Quitter", "Esci", "Sair"]),
        ("Agents Pixel Dungeon · {} necesitan atención", ["Agents Pixel Dungeon · {} need attention", "Agents Pixel Dungeon · {} demandent ton attention",
                                                       "Agents Pixel Dungeon · {} richiedono attenzione", "Agents Pixel Dungeon · {} precisam de atenção"]),
        ("Creado por Diabolo Studio (diabolostudio.com) · Nacho Valencia.\nCódigo y pixel art originales · MIT.\n\nProyecto independiente, sin afiliación con Herdr, Inc.",
            ["Created by Diabolo Studio (diabolostudio.com) · Nacho Valencia.\nOriginal code and pixel art · MIT.\n\nIndependent project, not affiliated with Herdr, Inc.",
             "Créé par Diabolo Studio (diabolostudio.com) · Nacho Valencia.\nCode et pixel art originaux · MIT.\n\nProjet indépendant, sans lien avec Herdr, Inc.",
             "Creato da Diabolo Studio (diabolostudio.com) · Nacho Valencia.\nCodice e pixel art originali · MIT.\n\nProgetto indipendente, non affiliato a Herdr, Inc.",
             "Criado por Diabolo Studio (diabolostudio.com) · Nacho Valencia.\nCódigo e pixel art originais · MIT.\n\nProjeto independente, sem afiliação com a Herdr, Inc."]),
        // Sounds and chat
        ("↑↓ y Enter eligen · esc cancela", ["↑↓ and Enter choose · esc cancels", "↑↓ et Enter choisissent · échap annule", "↑↓ e Enter scelgono · esc annulla", "↑↓ e Enter escolhem · esc cancela"]),
        ("↑↓ y Enter eligen · o responde a {}…", ["↑↓ and Enter choose · or reply to {}…", "↑↓ et Enter choisissent · ou réponds à {}…", "↑↓ e Enter scelgono · o rispondi a {}…", "↑↓ e Enter escolhem · ou responda a {}…"]),
        ("Finalizar agente (/exit)", ["End agent (/exit)", "Terminer l’agent (/exit)", "Termina l’agente (/exit)", "Encerrar agente (/exit)"]),
        ("¿Finalizar {}?", ["End {}?", "Terminer {} ?", "Terminare {}?", "Encerrar {}?"]),
        ("Se escribirá /exit en su chat de Herdr.", ["/exit will be typed into its chat in Herdr.", "/exit sera tapé dans son chat dans Herdr.", "/exit verrà scritto nella sua chat in Herdr.", "/exit será digitado no chat dele no Herdr."]),
        ("Finalizar", ["End", "Terminer", "Termina", "Encerrar"]),
        ("Cancelar", ["Cancel", "Annuler", "Annulla", "Cancelar"]),
        ("Herdr rechazó la acción. Revisa el panel del agente.", ["Herdr refused the action. Check the agent’s pane.", "Herdr a refusé l’action. Vérifie le panneau de l’agent.", "Herdr ha rifiutato l’azione. Controlla il pannello dell’agente.", "O Herdr recusou a ação. Verifique o painel do agente."]),
        ("Tú → {}: {}", ["You → {}: {}", "Toi → {} : {}", "Tu → {}: {}", "Você → {}: {}"]),
        ("Sonidos", ["Sounds", "Sons", "Suoni", "Sons"]),
        ("Sonidos activados", ["Sounds on", "Sons activés", "Suoni attivi", "Sons ativados"]),
        ("Al necesitar ayuda", ["When help is needed", "Quand il faut de l’aide", "Quando serve aiuto", "Ao precisar de ajuda"]),
        ("Al terminar", ["When finished", "À la fin", "Al termine", "Ao terminar"]),
        ("Silenciar sonidos", ["Mute sounds", "Couper les sons", "Silenzia i suoni", "Silenciar sons"]),
        ("Activar sonidos", ["Turn sounds on", "Activer les sons", "Attiva i suoni", "Ativar sons"]),
        ("Enfocar este agente en Herdr", ["Focus this agent in Herdr", "Afficher cet agent dans Herdr", "Metti a fuoco questo agente in Herdr", "Focar este agente no Herdr"]),
        ("Leyendo la terminal…", ["Reading the terminal…", "Lecture du terminal…", "Leggo il terminale…", "Lendo o terminal…"]),
        ("Escribir a {}…", ["Write to {}…", "Écrire à {}…", "Scrivi a {}…", "Escrever para {}…"]),
        ("Enviar (Enter)", ["Send (Enter)", "Envoyer (Enter)", "Invia (Enter)", "Enviar (Enter)"]),
        ("Aceptar", ["Accept", "Accepter", "Accetta", "Aceitar"]),
        ("Elegir la opción marcada del agente (Enter en su terminal)", ["Choose the agent’s highlighted option (Enter in its terminal)", "Choisir l’option sélectionnée de l’agent (Enter dans son terminal)", "Scegli l’opzione evidenziata dell’agente (Enter nel suo terminale)", "Escolher a opção marcada do agente (Enter no terminal dele)"]),
        ("Rechazar", ["Decline", "Refuser", "Rifiuta", "Recusar"]),
        ("Cancelar la petición del agente (esc en su terminal)", ["Cancel the agent’s request (esc in its terminal)", "Annuler la demande de l’agent (esc dans son terminal)", "Annulla la richiesta dell’agente (esc nel suo terminale)", "Cancelar o pedido do agente (esc no terminal dele)"]),
        ("Detener", ["Stop", "Arrêter", "Ferma", "Parar"]),
        ("Interrumpir al agente (esc en su terminal)", ["Interrupt the agent (esc in its terminal)", "Interrompre l’agent (esc dans son terminal)", "Interrompi l’agente (esc nel suo terminale)", "Interromper o agente (esc no terminal dele)"]),
        ("Sin agentes en la sesión", ["No agents in this session", "Aucun agent dans la session", "Nessun agente nella sessione", "Nenhum agente na sessão"]),
        // Herdr connection
        ("Herdr no entregó un snapshot válido.", ["Herdr did not return a valid snapshot.", "Herdr n'a pas renvoyé d'instantané valide.",
                                                 "Herdr non ha restituito uno snapshot valido.", "O Herdr não retornou um snapshot válido."]),
        ("No se encontró Herdr. Instálalo o define HERDR_BIN.", ["Herdr not found. Install it or set HERDR_BIN.", "Herdr introuvable. Installe-le ou définis HERDR_BIN.",
                                                                "Herdr non trovato. Installalo o imposta HERDR_BIN.", "Herdr não encontrado. Instale-o ou defina HERDR_BIN."]),
        ("Sin conexión con Herdr. Abre tu sesión; reintentamos automáticamente.",
            ["No connection to Herdr. Open your session; we retry automatically.",
             "Pas de connexion à Herdr. Ouvre ta session ; nouvel essai automatique.",
             "Nessuna connessione a Herdr. Apri la tua sessione; riproviamo automaticamente.",
             "Sem conexão com o Herdr. Abra sua sessão; tentamos de novo automaticamente."]),
        ("Sin título de actividad", ["No activity title", "Aucun titre d'activité", "Nessun titolo di attività", "Sem título de atividade"]),
        // Events
        ("Guild conectada · {} agentes", ["Guild connected · {} agents", "Guilde connectée · {} agents", "Gilda connessa · {} agenti", "Guilda conectada · {} agentes"]),
        ("{} · {} subagentes activos", ["{} · {} active subagents", "{} · {} sous-agents actifs", "{} · {} subagenti attivi", "{} · {} subagentes ativos"]),
        ("{} entró a la guild", ["{} joined the guild", "{} a rejoint la guilde", "{} è entrato nella gilda", "{} entrou na guilda"]),
        ("{} salió de la guild", ["{} left the guild", "{} a quitté la guilde", "{} ha lasciato la gilda", "{} saiu da guilda"]),
        // HUD: filters and search
        ("Ningún agente coincide", ["No agent matches", "Aucun agent ne correspond", "Nessun agente corrisponde", "Nenhum agente corresponde"]),
        ("Todos", ["All", "Tous", "Tutti", "Todos"]),
        ("Buscar…", ["Search…", "Rechercher…", "Cerca…", "Buscar…"]),
        ("Cerrar búsqueda (esc)", ["Close search (esc)", "Fermer la recherche (esc)", "Chiudi ricerca (esc)", "Fechar busca (esc)"]),
        ("Buscar por proyecto, agente, rama, carpeta o actividad", ["Search by project, agent, branch, folder or activity", "Rechercher par projet, agent, branche, dossier ou activité",
                                                                 "Cerca per progetto, agente, branch, cartella o attività", "Buscar por projeto, agente, branch, pasta ou atividade"]),
        // Notifications
        ("{} necesita atención", ["{} needs attention", "{} demande ton attention", "{} richiede attenzione", "{} precisa de atenção"]),
        ("{} terminó", ["{} finished", "{} a terminé", "{} ha finito", "{} terminou"]),
        ("Probar sonidos", ["Play the sounds", "Écouter les sons", "Prova i suoni", "Testar sons"]),
        ("Notificaciones", ["Notifications", "Notifications", "Notifiche", "Notificações"]),
        ("Notificaciones activadas", ["Notifications on", "Notifications activées", "Notifiche attive", "Notificações ativadas"]),
        // Background shells
        ("Descargar y contar 3000 artículos", ["Download and count 3000 articles", "Télécharger et compter 3000 articles", "Scarica e conta 3000 articoli", "Baixar e contar 3000 artigos"]),
        ("Ver su salida", ["Show its output", "Voir sa sortie", "Mostra il suo output", "Ver a sua saída"]),
        ("no", ["no", "non", "no", "não"]),
        ("sí, detener", ["yes, stop", "oui, arrêter", "sì, ferma", "sim, parar"]),
        ("Detener este shell", ["Stop this shell", "Arrêter ce shell", "Ferma questa shell", "Parar este shell"]),
        ("Sin salida todavía.", ["No output yet.", "Pas encore de sortie.", "Ancora nessun output.", "Ainda sem saída."]),
        ("Terminó.", ["Finished.", "Terminé.", "Finito.", "Terminou."]),
        ("{} · shell {}: {}", ["{} · shell {}: {}", "{} · shell {} : {}", "{} · shell {}: {}", "{} · shell {}: {}"]),
        ("{} · shell {}: detenido", ["{} · shell {}: stopped", "{} · shell {} : arrêté", "{} · shell {}: fermata", "{} · shell {}: parado"]),
        ("{}: un shell en segundo plano falló", ["{}: a background shell failed", "{} : un shell en arrière-plan a échoué", "{}: una shell in background è fallita", "{}: um shell em segundo plano falhou"]),
        ("{}: un shell en segundo plano terminó", ["{}: a background shell finished", "{} : un shell en arrière-plan a terminé", "{}: una shell in background è terminata", "{}: um shell em segundo plano terminou"]),
        // Activity log
        ("Registro de actividad", ["Activity log", "Journal d’activité", "Registro attività", "Registro de atividade"]),
        ("Mostrar en Finder: {}", ["Show in Finder: {}", "Afficher dans le Finder : {}", "Mostra nel Finder: {}", "Mostrar no Finder: {}"]),
        ("Abrir una terminal en esta carpeta", ["Open a terminal in this folder", "Ouvrir un terminal dans ce dossier", "Apri un terminale in questa cartella", "Abrir um terminal nesta pasta"]),
        ("Registro de la guild", ["Guild log", "Journal de la guilde", "Registro della gilda", "Registro da guilda"]),
        ("Aceptar lo que pide el agente: su opción marcada, o el «Sí» si la marcada es un «No»", ["Accept what the agent asks: its highlighted option, or the “Yes” when that one is a “No”", "Accepter ce que demande l’agent : son option en surbrillance, ou le « Oui » si celle-ci est un « Non »", "Accetta ciò che chiede l’agente: l’opzione evidenziata, o il «Sì» se quella è un «No»", "Aceitar o que o agente pede: a opção destacada, ou o «Sim» se ela for um «Não»"]),
        ("Cocinando en background", ["Brewing in the background", "Mijote en arrière-plan", "In cottura in background", "Cozinhando em segundo plano"]),
        ("Tamaño de las salas", ["Room size", "Taille des salles", "Dimensione delle stanze", "Tamanho das salas"]),
        ("Grande", ["Large", "Grande", "Grande", "Grande"]),
        ("Mediana", ["Medium", "Moyenne", "Media", "Média"]),
        ("Chica", ["Small", "Petite", "Piccola", "Pequena"]),
        ("{} archivos cambiados", ["{} files changed", "{} fichiers modifiés", "{} file modificati", "{} arquivos alterados"]),
        ("Leyendo cambios…", ["Reading changes…", "Lecture des modifications…", "Lettura delle modifiche…", "Lendo alterações…"]),
        ("No es un repositorio git", ["Not a git repository", "Pas un dépôt git", "Non è un repository git", "Não é um repositório git"]),
        ("Sin cambios", ["No changes", "Aucune modification", "Nessuna modifica", "Sem alterações"]),
        ("Elige un archivo para ver su diff", ["Pick a file to see its diff", "Choisissez un fichier pour voir son diff", "Scegli un file per vederne il diff", "Escolha um arquivo para ver o diff"]),
        ("Abrir el PR en el navegador", ["Open the PR in the browser", "Ouvrir la PR dans le navigateur", "Apri la PR nel browser", "Abrir o PR no navegador"]),
        ("abierto", ["open", "ouverte", "aperta", "aberto"]),
        ("mergeado", ["merged", "fusionnée", "unita", "mesclado"]),
        ("cerrado", ["closed", "fermée", "chiusa", "fechado"]),
        ("borrador", ["draft", "brouillon", "bozza", "rascunho"]),
        ("Los archivos que cambiaron en su carpeta y su diff, a la derecha", ["The files that changed in its folder and their diff, on the right", "Les fichiers modifiés dans son dossier et leur diff, à droite", "I file cambiati nella sua cartella e il loro diff, a destra", "Os arquivos alterados na pasta e o diff, à direita"]),
        ("Cerrar los archivos cambiados", ["Close the changed files", "Fermer les fichiers modifiés", "Chiudi i file modificati", "Fechar os arquivos alterados"]),
        ("Escudo del repo", ["The repo's emblem", "Blason du dépôt", "Stemma del repo", "Brasão do repo"]),
        ("Símbolo", ["Charge", "Meuble", "Simbolo", "Símbolo"]),
        ("Partición", ["Partition", "Partition", "Partizione", "Partição"]),
        ("Forma", ["Shape", "Forme", "Forma", "Forma"]),
        ("lo usa otro repo", ["another repo uses it", "un autre dépôt l’utilise", "lo usa un altro repo", "outro repo usa"]),
        ("Automático", ["Automatic", "Automatique", "Automatico", "Automático"]),
        ("Volver al escudo que sale del repo, con un color libre", ["Back to the emblem worked out from the repo, with a free colour", "Revenir au blason tiré du dépôt, avec une couleur libre", "Torna allo stemma ricavato dal repo, con un colore libero", "Voltar ao brasão do repo, com uma cor livre"]),
        ("Guardar", ["Save", "Enregistrer", "Salva", "Salvar"]),
        ("Clic derecho: editar el escudo", ["Right click: edit the emblem", "Clic droit : modifier le blason", "Clic destro: modifica lo stemma", "Clique direito: editar o brasão"]),
        ("Editar el escudo…", ["Edit the emblem…", "Modifier le blason…", "Modifica lo stemma…", "Editar o brasão…"]),
        ("Color principal", ["Main colour", "Couleur principale", "Colore principale", "Cor principal"]),
        ("Color secundario", ["Second colour", "Couleur secondaire", "Colore secondario", "Cor secundária"]),
        ("Sin commit", ["Uncommitted", "Non commité", "Non committato", "Sem commit"]),
        ("Sin subir", ["Unpushed", "Non poussé", "Non inviato", "Sem push"]),
        ("Contra main", ["Against main", "Contre main", "Contro main", "Contra main"]),
        ("contra {}", ["against {}", "contre {}", "contro {}", "contra {}"]),
        ("Lo que cambió desde el último commit", ["What changed since the last commit", "Ce qui a changé depuis le dernier commit", "Ciò che è cambiato dall’ultimo commit", "O que mudou desde o último commit"]),
        ("Todo lo que aún no está en el remoto: commits sin subir y cambios sin commit", ["Everything not on the remote yet: unpushed commits and uncommitted changes", "Tout ce qui n’est pas encore sur le dépôt distant : commits non poussés et modifications non commitées", "Tutto ciò che non è ancora sul remoto: commit non inviati e modifiche non committate", "Tudo o que ainda não está no remoto: commits sem push e alterações sem commit"]),
        ("Todo lo de la rama desde que salió de main, como en un PR", ["Everything on the branch since it left main, as in a PR", "Tout ce qui est sur la branche depuis main, comme dans une PR", "Tutto ciò che è sul ramo da quando ha lasciato main, come in una PR", "Tudo no branch desde que saiu da main, como num PR"]),
        ("Esta rama no está en el remoto", ["This branch is not on the remote", "Cette branche n’est pas sur le dépôt distant", "Questo ramo non è sul remoto", "Este branch não está no remoto"]),
        ("No hay rama main ni master", ["There is no main or master branch", "Pas de branche main ni master", "Non c’è un ramo main o master", "Não há branch main nem master"]),
        ("Te esperan", ["Waiting for you", "Ils vous attendent", "Ti aspettano", "Esperando por você"]),
        ("y {} más", ["and {} more", "et {} de plus", "e altri {}", "e mais {}"]),
        ("Abrir su chat", ["Open its chat", "Ouvrir son chat", "Apri la sua chat", "Abrir o chat"]),
        ("Aún no pasa nada.", ["Nothing has happened yet.", "Rien ne s’est encore passé.", "Non è ancora successo nulla.", "Nada aconteceu ainda."]),
        // Connection
        ("Esperando a Herdr…", ["Waiting for Herdr…", "En attente de Herdr…", "In attesa di Herdr…", "Aguardando o Herdr…"]),
        ("hace {} s", ["{} s ago", "il y a {} s", "{} s fa", "há {} s"]),
        ("hace {} min", ["{} min ago", "il y a {} min", "{} min fa", "há {} min"]),
        ("Herdr desconectado", ["Herdr disconnected", "Herdr déconnecté", "Herdr disconnesso", "Herdr desconectado"]),
        ("última actualización {}", ["last update {}", "dernière mise à jour {}", "ultimo aggiornamento {}", "última atualização {}"]),
        ("Datos sin actualizar · última actualización {}", ["Data not updating · last update {}", "Données figées · dernière mise à jour {}",
                                                            "Dati non aggiornati · ultimo aggiornamento {}", "Dados sem atualizar · última atualização {}"]),
        ("Reintentando la conexión automáticamente.", ["Retrying the connection automatically.", "Reconnexion automatique en cours.", "Riprovo la connessione automaticamente.", "Tentando reconectar automaticamente."]),
        ("Herdr no ha entregado datos nuevos.", ["Herdr has not sent new data.", "Herdr n’a pas envoyé de nouvelles données.", "Herdr non ha inviato dati nuovi.", "O Herdr não enviou dados novos."]),
        // Sessions
        ("Sesión de Herdr", ["Herdr session", "Session Herdr", "Sessione Herdr", "Sessão do Herdr"]),
        ("{} (detenida)", ["{} (stopped)", "{} (arrêtée)", "{} (ferma)", "{} (parada)"]),
        ("Demo (agentes ficticios)", ["Demo (fictional agents)", "Démo (agents fictifs)", "Demo (agenti fittizi)", "Demo (agentes fictícios)"]),
        // New agent
        ("Invocar un agente nuevo", ["Summon a new agent", "Invoquer un nouvel agent", "Evoca un nuovo agente", "Invocar um novo agente"]),
        ("Invocar un agente", ["Summon an agent", "Invoquer un agent", "Evoca un agente", "Invocar um agente"]),
        ("Elegir carpeta…", ["Choose folder…", "Choisir un dossier…", "Scegli cartella…", "Escolher pasta…"]),
        ("Carpeta…", ["Folder…", "Dossier…", "Cartella…", "Pasta…"]),
        ("Primer prompt (opcional)", ["First prompt (optional)", "Premier prompt (facultatif)", "Primo prompt (facoltativo)", "Primeiro prompt (opcional)"]),
        ("Invocando…", ["Summoning…", "Invocation…", "Evocazione…", "Invocando…"]),
        ("Invocar", ["Summon", "Invoquer", "Evoca", "Invocar"]),
        ("Crear el agente (⌘Enter)", ["Create the agent (⌘Enter)", "Créer l’agent (⌘Enter)", "Crea l’agente (⌘Enter)", "Criar o agente (⌘Enter)"]),
        ("La carpeta no existe.", ["The folder does not exist.", "Le dossier n’existe pas.", "La cartella non esiste.", "A pasta não existe."]),
        ("Herdr no devolvió el panel nuevo.", ["Herdr did not return the new pane.", "Herdr n’a pas renvoyé le nouveau panneau.", "Herdr non ha restituito il nuovo pannello.", "O Herdr não devolveu o novo painel."]),
        ("{} no arrancó. Revisa que esté instalado.", ["{} did not start. Check that it is installed.", "{} n’a pas démarré. Vérifie qu’il est installé.", "{} non è partito. Controlla che sia installato.", "{} não iniciou. Verifique se está instalado."]),
        ("{} no arrancó. La terminal nueva quedó en: «{}»", ["{} did not start. The new terminal was left at: “{}”", "{} n’a pas démarré. Le nouveau terminal est resté sur : « {} »", "{} non è partito. Il nuovo terminale è rimasto su: «{}»", "{} não iniciou. O novo terminal ficou em: «{}»"]),
        ("oh-my-zsh pregunta si actualizarse y se come el comando. Corre omz update en una terminal (o pon zstyle ':omz:update' mode auto en ~/.zshrc) y vuelve a invocar.", ["oh-my-zsh asks whether to update and eats the command. Run omz update in a terminal (or put zstyle ':omz:update' mode auto in ~/.zshrc) and summon again.", "oh-my-zsh demande s’il doit se mettre à jour et avale la commande. Lance omz update dans un terminal (ou mets zstyle ':omz:update' mode auto dans ~/.zshrc) et invoque à nouveau.", "oh-my-zsh chiede se aggiornarsi e si mangia il comando. Esegui omz update in un terminale (o metti zstyle ':omz:update' mode auto in ~/.zshrc) e invoca di nuovo.", "O oh-my-zsh pergunta se deve se atualizar e engole o comando. Rode omz update num terminal (ou coloque zstyle ':omz:update' mode auto no ~/.zshrc) e invoque de novo."]),
        ("zsh preguntó si corregir «{}»: algo en la shell nueva se comió parte del comando. Abre una terminal y revisa qué pregunta al arrancar.", ["zsh asked whether to correct “{}”: something in the new shell ate part of the command. Open a terminal and check what it asks at startup.", "zsh a proposé de corriger « {} » : quelque chose dans le nouveau shell a avalé une partie de la commande. Ouvre un terminal et regarde ce qu’il demande au démarrage.", "zsh ha chiesto se correggere «{}»: qualcosa nella nuova shell si è mangiato parte del comando. Apri un terminale e guarda cosa chiede all’avvio.", "O zsh perguntou se corrigia «{}»: algo no novo shell engoliu parte do comando. Abra um terminal e veja o que ele pergunta ao iniciar."]),
        ("El agente está esperando una respuesta; contéstale primero.", ["The agent is waiting for an answer; reply to it first.", "L’agent attend une réponse ; réponds-lui d’abord.", "L’agente aspetta una risposta; rispondigli prima.", "O agente está esperando uma resposta; responda primeiro."]),
        ("{} no quedó listo; su prompt no se envió.", ["{} never got ready; its prompt was not sent.", "{} n’a jamais été prêt ; son prompt n’a pas été envoyé.", "{} non è stato pronto; il prompt non è stato inviato.", "{} não ficou pronto; o prompt não foi enviado."]),
        ("{} no recibió su prompt.", ["{} did not get its prompt.", "{} n’a pas reçu son prompt.", "{} non ha ricevuto il prompt.", "{} não recebeu o prompt."]),
        ("{} espera tu respuesta; su prompt se enviará cuando esté listo.", ["{} is waiting for your answer; its prompt will be sent once it is ready.", "{} attend ta réponse ; son prompt partira dès qu’il sera prêt.", "{} aspetta la tua risposta; il prompt partirà appena è pronto.", "{} espera sua resposta; o prompt será enviado quando estiver pronto."]),
        ("El agente arrancó, pero no recibió el prompt.", ["The agent started but did not get the prompt.", "L’agent a démarré mais n’a pas reçu le prompt.", "L’agente è partito ma non ha ricevuto il prompt.", "O agente iniciou, mas não recebeu o prompt."]),
        ("Invocaste a {} en {}", ["You summoned {} in {}", "Tu as invoqué {} dans {}", "Hai evocato {} in {}", "Você invocou {} em {}"]),
        // Window preferences
        ("Siempre visible", ["Always on top", "Toujours au premier plan", "Sempre in primo piano", "Sempre visível"]),
        ("Abrir al iniciar sesión", ["Open at login", "Ouvrir à la connexion", "Apri al login", "Abrir ao iniciar sessão"]),
        ("El sistema no permitió abrir la app al iniciar sesión.", ["The system did not allow opening the app at login.", "Le système n’a pas autorisé l’ouverture à la connexion.",
                                                                   "Il sistema non ha permesso l’apertura al login.", "O sistema não permitiu abrir o app ao iniciar sessão."]),
        // Standalone mode and Herdr
        ("Herdr no está instalado · modo autónomo, solo lectura", ["Herdr is not installed · standalone, read-only", "Herdr n’est pas installé · mode autonome, lecture seule", "Herdr non è installato · modalità autonoma, sola lettura", "O Herdr não está instalado · modo autônomo, somente leitura"]),
        ("Herdr no está corriendo · modo autónomo, solo lectura", ["Herdr is not running · standalone, read-only", "Herdr ne tourne pas · mode autonome, lecture seule", "Herdr non è in esecuzione · modalità autonoma, sola lettura", "O Herdr não está rodando · modo autônomo, somente leitura"]),
        ("Instalar Herdr", ["Install Herdr", "Installer Herdr", "Installa Herdr", "Instalar o Herdr"]),
        ("Abrir Herdr", ["Open Herdr", "Ouvrir Herdr", "Apri Herdr", "Abrir o Herdr"]),
        ("Seguir sin Herdr", ["Carry on without Herdr", "Continuer sans Herdr", "Continua senza Herdr", "Seguir sem o Herdr"]),
        ("Instalando Herdr…", ["Installing Herdr…", "Installation de Herdr…", "Installo Herdr…", "Instalando o Herdr…"]),
        ("Herdr instalado. Ábrelo y lanza tus agentes dentro.", ["Herdr installed. Open it and run your agents inside.", "Herdr installé. Ouvre-le et lance tes agents dedans.", "Herdr installato. Aprilo e avvia i tuoi agenti dentro.", "Herdr instalado. Abra-o e rode seus agentes dentro."]),
        ("La instalación falló: {}", ["The install failed: {}", "L’installation a échoué : {}", "L’installazione è fallita: {}", "A instalação falhou: {}"]),
        ("No se pudo abrir una terminal con Herdr.", ["Could not open a terminal with Herdr.", "Impossible d’ouvrir un terminal avec Herdr.", "Impossibile aprire un terminale con Herdr.", "Não foi possível abrir um terminal com o Herdr."]),
        ("Abrir Herdr al iniciar", ["Open Herdr at startup", "Ouvrir Herdr au démarrage", "Apri Herdr all’avvio", "Abrir o Herdr ao iniciar"]),
        ("Instala Herdr para responder desde aquí.", ["Install Herdr to reply from here.", "Installe Herdr pour répondre d’ici.", "Installa Herdr per rispondere da qui.", "Instale o Herdr para responder daqui."]),
        ("Los agentes se ven, pero no se les puede hablar: Herdr no está.", ["Agents are shown but cannot be talked to: Herdr is missing.", "Les agents sont visibles mais on ne peut pas leur parler : Herdr manque.", "Gli agenti si vedono ma non si può parlare loro: manca Herdr.", "Os agentes aparecem, mas não dá para falar com eles: falta o Herdr."]),
        // Full screen
        ("Pantalla completa", ["Full screen", "Plein écran", "Schermo intero", "Tela cheia"]),
        ("Salir de pantalla completa", ["Exit full screen", "Quitter le plein écran", "Esci da schermo intero", "Sair da tela cheia"]),
        ("Elige una sala para ver su consola", ["Pick a room to see its console", "Choisis une salle pour voir sa console", "Scegli una stanza per vedere la sua console", "Escolha uma sala para ver seu console"]),
        ("esc vuelve a la ventana", ["esc goes back to the window", "esc revient à la fenêtre", "esc torna alla finestra", "esc volta à janela"]),
        // Text size
        ("Tamaño del texto", ["Text size", "Taille du texte", "Dimensione del testo", "Tamanho do texto"]),
        ("Más grande", ["Bigger", "Plus grand", "Più grande", "Maior"]),
        ("Más pequeño", ["Smaller", "Plus petit", "Più piccolo", "Menor"]),
        ("Normal", ["Normal", "Normale", "Normale", "Normal"]),
        ("Suelta para añadir la ruta al mensaje", ["Drop to add the path to the message", "Dépose pour ajouter le chemin au message", "Rilascia per aggiungere il percorso al messaggio", "Solte para adicionar o caminho à mensagem"]),
        ("Tamaño de la consola", ["Console text size", "Taille du texte de la console", "Dimensione del testo della console", "Tamanho do texto do console"]),
        ("Consola más grande", ["Bigger console text", "Console plus grande", "Console più grande", "Console maior"]),
        ("Consola más pequeña", ["Smaller console text", "Console plus petite", "Console più piccola", "Console menor"]),
        // Demo activity titles
        ("Construyendo la página de ajustes", ["Building the settings page", "Construction de la page des réglages", "Costruisco la pagina delle impostazioni", "Construindo a página de ajustes"]),
        ("Revisando las pruebas", ["Reviewing the tests", "Révision des tests", "Rivedo i test", "Revisando os testes"]),
        ("Esperando tu respuesta", ["Waiting for your answer", "En attente de ta réponse", "In attesa della tua risposta", "Aguardando sua resposta"]),
        ("Leyendo archivos del proyecto…", ["Reading project files…", "Lecture des fichiers du projet…", "Lettura dei file del progetto…", "Lendo os arquivos do projeto…"]),
        ("Listo para la próxima tarea", ["Ready for the next task", "Prêt pour la prochaine tâche", "Pronto per il prossimo compito", "Pronto para a próxima tarefa"]),
        ("Documentación actualizada", ["Documentation updated", "Documentation à jour", "Documentazione aggiornata", "Documentação atualizada"]),
        ("Ajustando el movimiento", ["Tuning the movement", "Réglage du mouvement", "Regolo il movimento", "Ajustando o movimento"]),
    ];
    rows.iter().copied().collect()
});

/// The UI language: the first preferred system locale we have a table for,
/// else English. `HPD_LANG` overrides it (handy for screenshots).
pub static CURRENT: LazyLock<&'static str> = LazyLock::new(|| {
    if let Ok(forced) = std::env::var("HPD_LANG") {
        if let Some(code) = LANGUAGES.iter().find(|l| forced.to_lowercase().starts_with(**l)) {
            return code;
        }
    }
    for locale in sys_locale::get_locales() {
        let code = locale.get(..2).unwrap_or("").to_lowercase();
        if let Some(found) = LANGUAGES.iter().find(|l| **l == code) {
            return found;
        }
    }
    "en"
});

/// The text of a Spanish source string in `language`.
pub fn text(spanish: &str, language: &str) -> String {
    if language == "es" {
        return spanish.to_string();
    }
    let Some(index) = LANGUAGES.iter().position(|l| *l == language) else { return spanish.to_string() };
    match TABLE.get(spanish) {
        Some(row) if index >= 1 => row[index - 1].to_string(),
        _ => spanish.to_string(),
    }
}

/// Translate a Spanish UI string.
pub fn tr(spanish: &str) -> String {
    text(spanish, &CURRENT)
}

/// Translate a Spanish UI string and fill its `{}` placeholders in order.
pub fn trf(spanish: &str, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = tr(spanish);
    for arg in args {
        if let Some(at) = out.find("{}") {
            out.replace_range(at..at + 2, &arg.to_string());
        }
    }
    out
}
