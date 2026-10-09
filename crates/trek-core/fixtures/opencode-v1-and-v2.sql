CREATE TABLE `session` (
          `id` text PRIMARY KEY,
          `project_id` text NOT NULL,
          `workspace_id` text,
          `parent_id` text,
          `slug` text NOT NULL,
          `directory` text NOT NULL,
          `path` text,
          `title` text NOT NULL,
          `version` text NOT NULL,
          `share_url` text,
          `summary_additions` integer,
          `summary_deletions` integer,
          `summary_files` integer,
          `summary_diffs` text,
          `metadata` text,
          `cost` real DEFAULT 0 NOT NULL,
          `tokens_input` integer DEFAULT 0 NOT NULL,
          `tokens_output` integer DEFAULT 0 NOT NULL,
          `tokens_reasoning` integer DEFAULT 0 NOT NULL,
          `tokens_cache_read` integer DEFAULT 0 NOT NULL,
          `tokens_cache_write` integer DEFAULT 0 NOT NULL,
          `revert` text,
          `permission` text,
          `agent` text,
          `model` text,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `time_compacting` integer,
          `time_archived` integer,
          CONSTRAINT `fk_session_project_id_project_id_fk` FOREIGN KEY (`project_id`) REFERENCES `project`(`id`) ON DELETE CASCADE
        );
INSERT INTO session VALUES('ses_edfc986b6ffe4mZ9ccSLf9DwLP','global',NULL,NULL,'sunny-planet','/Users/me/app','private/Users/me/app','Counting directory entries with ls','1.18.35',NULL,0,0,0,NULL,NULL,0.0,7800,22,19,7680,0,NULL,NULL,'build','{"id":"mimo-v2.6-flash-free","providerID":"opencode","variant":"default"}',1791541803337,1791541811322,NULL,NULL);
CREATE TABLE `message` (
          `id` text PRIMARY KEY,
          `session_id` text NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `data` text NOT NULL,
          CONSTRAINT `fk_message_session_id_session_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session`(`id`) ON DELETE CASCADE
        );
INSERT INTO message VALUES('msg_120367977001qxZJyJ3aBjbODa','ses_edfc986b6ffe4mZ9ccSLf9DwLP',1791541803383,1791541811325,'{"role":"user","time":{"created":1791541803383},"agent":"build","model":{"providerID":"opencode","modelID":"mimo-v2.6-flash-free"},"summary":{"diffs":[]}}');
INSERT INTO message VALUES('msg_1203679a1001Jb1Bc4H1Wm8fSs','ses_edfc986b6ffe4mZ9ccSLf9DwLP',1791541803425,1791541806658,'{"parentID":"msg_120367977001qxZJyJ3aBjbODa","role":"assistant","mode":"build","agent":"build","path":{"cwd":"/Users/me/app","root":"/"},"cost":0,"tokens":{"total":7752,"input":7718,"output":19,"reasoning":15,"cache":{"write":0,"read":0}},"modelID":"mimo-v2.6-flash-free","providerID":"opencode","time":{"created":1791541803425,"completed":1791541806657},"finish":"tool-calls"}');
INSERT INTO message VALUES('msg_120368645001x2UKfpq78vze2p','ses_edfc986b6ffe4mZ9ccSLf9DwLP',1791541806661,1791541811317,'{"parentID":"msg_120367977001qxZJyJ3aBjbODa","role":"assistant","mode":"build","agent":"build","path":{"cwd":"/Users/me/app","root":"/"},"cost":0,"tokens":{"total":7769,"input":82,"output":3,"reasoning":4,"cache":{"write":0,"read":7680}},"modelID":"mimo-v2.6-flash-free","providerID":"opencode","time":{"created":1791541806661,"completed":1791541811317},"finish":"stop"}');
CREATE TABLE `part` (
          `id` text PRIMARY KEY,
          `message_id` text NOT NULL,
          `session_id` text NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `data` text NOT NULL,
          CONSTRAINT `fk_part_message_id_message_id_fk` FOREIGN KEY (`message_id`) REFERENCES `message`(`id`) ON DELETE CASCADE
        );
INSERT INTO part VALUES('prt_12036797e001kMj3OdbzhJRLBs','msg_120367977001qxZJyJ3aBjbODa','ses_edfc986b6ffe4mZ9ccSLf9DwLP',1791541803398,1791541803399,'{"type":"text","text":"Use your bash tool to run `ls`, then reply with just the number of entries it printed."}');
INSERT INTO part VALUES('prt_120368488001iij3iBq6ucdows','msg_1203679a1001Jb1Bc4H1Wm8fSs','ses_edfc986b6ffe4mZ9ccSLf9DwLP',1791541806216,1791541806217,'{"type":"step-start"}');
INSERT INTO part VALUES('prt_12036848a001HaGQfmABOAsiZJ','msg_1203679a1001Jb1Bc4H1Wm8fSs','ses_edfc986b6ffe4mZ9ccSLf9DwLP',1791541806218,1791541806343,'{"type":"reasoning","text":"The user wants me to run `ls` and count the entries.","time":{"start":1791541806218,"end":1791541806342}}');
INSERT INTO part VALUES('prt_1203685090013buphdE3F6WJta','msg_1203679a1001Jb1Bc4H1Wm8fSs','ses_edfc986b6ffe4mZ9ccSLf9DwLP',1791541806345,1791541806640,'{"type":"tool","tool":"bash","callID":"call_be9561b338cc48d3923356e5","state":{"status":"completed","input":{"command":"ls"},"output":"notes.txt\n","metadata":{"output":"notes.txt\n","exit":0,"truncated":false},"title":"ls","time":{"start":1791541806385,"end":1791541806639}}}');
INSERT INTO part VALUES('prt_1203686350012EUUqXu55fal3E','msg_1203679a1001Jb1Bc4H1Wm8fSs','ses_edfc986b6ffe4mZ9ccSLf9DwLP',1791541806646,1791541806647,'{"reason":"tool-calls","type":"step-finish","tokens":{"total":7752,"input":7718,"output":19,"reasoning":15,"cache":{"write":0,"read":0}},"cost":0}');
INSERT INTO part VALUES('prt_1203697ea001WIepimuh5AkcIU','msg_120368645001x2UKfpq78vze2p','ses_edfc986b6ffe4mZ9ccSLf9DwLP',1791541811178,1791541811179,'{"type":"step-start"}');
INSERT INTO part VALUES('prt_1203697ec001LbkP5mKRFRSw2X','msg_120368645001x2UKfpq78vze2p','ses_edfc986b6ffe4mZ9ccSLf9DwLP',1791541811180,1791541811298,'{"type":"reasoning","text":"One entry.","time":{"start":1791541811180,"end":1791541811297}}');
INSERT INTO part VALUES('prt_120369863001uh77LrV0ZZG0D3','msg_120368645001x2UKfpq78vze2p','ses_edfc986b6ffe4mZ9ccSLf9DwLP',1791541811299,1791541811303,'{"type":"text","text":"1","time":{"start":1791541811299,"end":1791541811301}}');
INSERT INTO part VALUES('prt_12036986800187TD2MoN1MdEFG','msg_120368645001x2UKfpq78vze2p','ses_edfc986b6ffe4mZ9ccSLf9DwLP',1791541811304,1791541811310,'{"reason":"stop","type":"step-finish","tokens":{"total":7769,"input":82,"output":3,"reasoning":4,"cache":{"write":0,"read":7680}},"cost":0}');
CREATE TABLE `session_v2` (
          `id` text PRIMARY KEY,
          `project_id` text NOT NULL,
          `workspace_id` text,
          `parent_id` text,
          `fork_session_id` text,
          `fork_boundary` text,
          `slug` text NOT NULL,
          `directory` text NOT NULL,
          `path` text,
          `title` text,
          `version` text NOT NULL,
          `share_url` text,
          `summary_additions` integer,
          `summary_deletions` integer,
          `summary_files` integer,
          `summary_diffs` text,
          `metadata` text,
          `cost` real DEFAULT 0 NOT NULL,
          `tokens_input` integer DEFAULT 0 NOT NULL,
          `tokens_output` integer DEFAULT 0 NOT NULL,
          `tokens_reasoning` integer DEFAULT 0 NOT NULL,
          `tokens_cache_read` integer DEFAULT 0 NOT NULL,
          `tokens_cache_write` integer DEFAULT 0 NOT NULL,
          `revert` text,
          `permission` text,
          `agent` text,
          `model` text,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `time_compacting` integer,
          `time_archived` integer,
          `time_suspended` integer, `resume_attempts` integer DEFAULT 0 NOT NULL, `time_idle` integer, `time_viewed` integer, `idle_outcome` text,
          CONSTRAINT `fk_session_v2_project_id_project_id_fk` FOREIGN KEY (`project_id`) REFERENCES `project`(`id`) ON DELETE CASCADE
        );
INSERT INTO session_v2 VALUES('ses_edfc986b6ffe4mZ9ccSLf9DwLP','global',NULL,NULL,NULL,NULL,'sunny-planet','/Users/me/app','private/Users/me/app','Counting directory entries with ls','1.18.35',NULL,0,0,0,NULL,NULL,0.0,7800,22,19,7680,0,NULL,NULL,'build','{"id":"mimo-v2.6-flash-free","providerID":"opencode","variant":"default"}',1791541803337,1791541811322,NULL,NULL,NULL,0,NULL,NULL,NULL);
INSERT INTO session_v2 VALUES('ses_edfcbb35fffervIIJpuPw7hX2w','ff67df61798f94040cae14106dfac8605695a32b',NULL,NULL,NULL,NULL,'eager-island','/Users/me/app','../../../../Users/me/app','Running ls and counting entries','2.0.26',NULL,NULL,NULL,NULL,NULL,NULL,0.0,554,29,329,10048,0,NULL,NULL,NULL,'{"id":"mimo-v2.6-flash-free","providerID":"opencode"}',1791541660838,1791541675204,NULL,NULL,NULL,0,1791541675206,NULL,'succeeded');
INSERT INTO session_v2 VALUES('ses_edfcbd056ffeGuaDEh9Roa4ew5','ff67df61798f94040cae14106dfac8605695a32b',NULL,NULL,NULL,NULL,'stellar-sailor','/Users/me/app','../../../../Users/me/app',NULL,'2.0.26',NULL,NULL,NULL,NULL,NULL,NULL,0.0,0,0,0,0,0,NULL,NULL,NULL,NULL,1791541653450,1791541653886,NULL,NULL,NULL,0,1791541653888,NULL,'failed');
CREATE TABLE IF NOT EXISTS "session_message" (
          `id` text PRIMARY KEY,
          `session_id` text NOT NULL,
          `type` text NOT NULL,
          `seq` integer NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `data` text NOT NULL,
          CONSTRAINT `fk_session_message_session_id_session_v2_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session_v2`(`id`) ON DELETE CASCADE
        );
INSERT INTO session_message VALUES('msg_120367977001qxZJyJ3aBjbODa','ses_edfc986b6ffe4mZ9ccSLf9DwLP','user',0,1791541803383,1791541811325,'{"text":"Use your bash tool to run `ls`, then reply with just the number of entries it printed.","time":{"created":1791541803383}}');
INSERT INTO session_message VALUES('msg_1203679a1001Jb1Bc4H1Wm8fSs','ses_edfc986b6ffe4mZ9ccSLf9DwLP','assistant',1,1791541803425,1791541806658,'{"agent":"build","model":{"providerID":"opencode","id":"mimo-v2.6-flash-free","variant":"default"},"content":[{"type":"reasoning","text":"The user wants me to run `ls` and count the entries.","time":{"created":1791541806218,"completed":1791541806342}},{"type":"tool","id":"call_be9561b338cc48d3923356e5","name":"bash","state":{"status":"completed","input":{"command":"ls"},"content":[{"type":"text","text":"notes.txt\n"}],"metadata":{"output":"notes.txt\n","exit":0,"truncated":false}},"time":{"created":1791541806385,"completed":1791541806639}}],"finish":"tool-calls","cost":0,"tokens":{"input":7718,"output":19,"reasoning":15,"cache":{"read":0,"write":0}},"time":{"created":1791541803425,"completed":1791541806658}}');
INSERT INTO session_message VALUES('msg_120368645001x2UKfpq78vze2p','ses_edfc986b6ffe4mZ9ccSLf9DwLP','assistant',2,1791541806661,1791541811317,'{"agent":"build","model":{"providerID":"opencode","id":"mimo-v2.6-flash-free","variant":"default"},"content":[{"type":"reasoning","text":"One entry.","time":{"created":1791541811180,"completed":1791541811297}},{"type":"text","text":"1"}],"finish":"stop","cost":0,"tokens":{"input":82,"output":3,"reasoning":4,"cache":{"read":7680,"write":0}},"time":{"created":1791541806661,"completed":1791541811317}}');
INSERT INTO session_message VALUES('msg_120368645001vKZIOjSys1HgY6','ses_edfc986b6ffe4mZ9ccSLf9DwLP','system',3,1791541806661,1791541811317,'{"text":"The available tools have changed.\n\nThe `bash` tool is now `shell` and must be called by that name.","time":{"created":1791541806661}}');
INSERT INTO session_message VALUES('msg_120344cb2001BFqhc3Du19qmx2','ses_edfcbb35fffervIIJpuPw7hX2w','model-switched',1,1791541660850,1791541660851,'{"time":{"created":1791541660850},"model":{"id":"mimo-v2.6-flash-free","providerID":"opencode"}}');
INSERT INTO session_message VALUES('msg_120344cbb001QojVrJkeNtoNXG','ses_edfcbb35fffervIIJpuPw7hX2w','user',5,1791541660925,1791541660926,'{"time":{"created":1791541660925},"text":"Use your bash tool to run `ls`, then reply with just the number of entries it printed.","files":[]}');
INSERT INTO session_message VALUES('msg_120344d0d0010Hlulf51Gh97vC','ses_edfcbb35fffervIIJpuPw7hX2w','assistant',8,1791541661001,1791541668637,'{"time":{"created":1791541661001,"streamed":1791541668586,"completed":1791541668636},"agent":"build","model":{"id":"mimo-v2.6-flash-free","providerID":"opencode","variant":"default"},"content":[{"type":"reasoning","text":"The user wants me to run `ls` and reply with just the number of entries printed. Let me use the execute tool to run it.\n\nI should use the execute tool with tools.shell presumably. Wait, the catalog says opencode (5 tools) only... but the execute tool says \"Available tools: opencode (5 tools)\". Hmm, but the system says \"Use the `execute` tool to call the tools listed below.\" The listed tools are only opencode tools. But the user asks to run `ls` with bash tool. Maybe there''s a shell tool available via execute? The catalog only lists opencode tools. Hmm.\n\nActually, maybe I can call tools.shell inside execute even though not listed? The instructions say don''t guess tool names. But the shell tool is defined at top level (in my available functions). Actually, looking at my function list, I have: edit, glob, grep, question, read, shell, skill, subagent, webfetch, websearch, execute. So `shell` is a top-level tool! I can call it directly.\n\nLet me call shell with `ls`.","state":{"reasoningField":"reasoning_content","reasoningDetails":[{"type":"reasoning.text","text":"The user wants me to run `ls` and reply with just the number of entries printed. Let me use the execute tool to run it.\n\nI should use the execute tool with tools.shell presumably. Wait, the catalog says opencode (5 tools) only... but the execute tool says \"Available tools: opencode (5 tools)\". Hmm, but the system says \"Use the `execute` tool to call the tools listed below.\" The listed tools are only opencode tools. But the user asks to run `ls` with bash tool. Maybe there''s a shell tool available via execute? The catalog only lists opencode tools. Hmm.\n\nActually, maybe I can call tools.shell inside execute even though not listed? The instructions say don''t guess tool names. But the shell tool is defined at top level (in my available functions). Actually, looking at my function list, I have: edit, glob, grep, question, read, shell, skill, subagent, webfetch, websearch, execute. So `shell` is a top-level tool! I can call it directly.\n\nLet me call shell with `ls`.","format":"unknown","index":0}]},"time":{"created":1791541664892,"completed":1791541668580}},{"type":"tool","id":"call_63e33f94ef1f4abd8bff6720","name":"shell","executed":false,"state":{"status":"completed","input":{"command":"ls"},"content":[{"type":"text","text":"notes.txt\n"}],"metadata":{"status":"completed","truncated":false,"exit":0}},"time":{"created":1791541668516,"ran":1791541668584,"completed":1791541668633}}],"finish":"tool-calls","rawFinish":"tool_calls","cost":0,"tokens":{"input":230,"output":19,"reasoning":230,"cache":{"read":4672,"write":0}}}');
INSERT INTO session_message VALUES('msg_120346b270019H7yboh4modGoy','ses_edfcbb35fffervIIJpuPw7hX2w','assistant',17,1791541668663,1791541675205,'{"time":{"created":1791541668663,"streamed":1791541675203,"completed":1791541675204},"agent":"build","model":{"id":"mimo-v2.6-flash-free","providerID":"opencode","variant":"default"},"content":[{"type":"reasoning","text":"The output printed one entry: notes.txt. Wait, but the working directory /Users/me/app — ls printed just \"notes.txt\". So the number is 1.\n\nBut maybe I should double check — ls output might be truncated? It shows just one line. Let me count: 1 entry.\n\nThe user wants just the number.","state":{"reasoningField":"reasoning_content","reasoningDetails":[{"type":"reasoning.text","text":"The output printed one entry: notes.txt. Wait, but the working directory /Users/me/app — ls printed just \"notes.txt\". So the number is 1.\n\nBut maybe I should double check — ls output might be truncated? It shows just one line. Let me count: 1 entry.\n\nThe user wants just the number.","format":"unknown","index":0}]},"time":{"created":1791541674137,"completed":1791541675200}},{"type":"text","text":"1"}],"finish":"stop","rawFinish":"stop","cost":0,"tokens":{"input":297,"output":3,"reasoning":74,"cache":{"read":4864,"write":0}}}');
INSERT INTO session_message VALUES('msg_1203484c6001URWQSfIiCixQHn','ses_edfcbb35fffervIIJpuPw7hX2w','idle',24,1791541675206,1791541675206,'{"time":{"created":1791541675206},"outcome":"succeeded"}');
INSERT INTO session_message VALUES('msg_120342fdf001ClSN402qfc51lT','ses_edfcbd056ffeGuaDEh9Roa4ew5','user',4,1791541653494,1791541653495,'{"time":{"created":1791541653494},"text":"Use your bash tool to run `ls`, then reply with just the number of entries it printed.","files":[]}');
INSERT INTO session_message VALUES('msg_1203430070015WfbSTa0sMA6p0','ses_edfcbd056ffeGuaDEh9Roa4ew5','assistant',5,1791541653521,1791541653887,'{"time":{"created":1791541653521,"completed":1791541653886},"agent":"build","model":{"id":"exo-free","providerID":"opencode"},"content":[],"finish":"error","error":{"type":"provider.invalid-request","message":"Model exo-free has been deprecated.","status":410,"response":{"body":"{\"type\":\"error\",\"error\":{\"type\":\"ModelDeprecated\",\"message\":\"Model exo-free has been deprecated.\"},\"metadata\":{\"model\":\"exo-free\"}}"}}}');
INSERT INTO session_message VALUES('msg_120343180001M3ZasqX2UObNMp','ses_edfcbd056ffeGuaDEh9Roa4ew5','idle',7,1791541653888,1791541653889,'{"time":{"created":1791541653888},"outcome":"failed"}');
CREATE UNIQUE INDEX `session_message_session_seq_idx` ON `session_message` (`session_id`,`seq`);
INSERT INTO session_v2 (id, project_id, workspace_id, parent_id, fork_session_id, fork_boundary, slug, directory, path, title, version, share_url, summary_additions, summary_deletions, summary_files, summary_diffs, metadata, cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write, revert, permission, agent, model, time_created, time_updated, time_idle, time_viewed, idle_outcome, time_compacting, time_archived, time_suspended, resume_attempts) VALUES('ses_edee57470ffeJ8L7m282BCBa0v','ff67df61798f94040cae14106dfac8605695a32b',NULL,NULL,NULL,NULL,'crisp-knight','/Users/me/app','../../../../Users/me/app',NULL,'0.0.0-beta-19296',NULL,NULL,NULL,NULL,NULL,NULL,0.0,6363,3,0,0,0,NULL,NULL,'build','{"id":"big-pickle","providerID":"opencode"}',1791556750225,1791556750310,1791556751951,NULL,'succeeded',NULL,NULL,NULL,0);
INSERT INTO session_message VALUES('msg_1211a8b9c001KCAlLAb9UQTjgF','ses_edee57470ffeJ8L7m282BCBa0v','model-switched',1,1791556750236,1791556750237,'{"time":{"created":1791556750236},"model":{"id":"big-pickle","providerID":"opencode"},"previous":{"id":"exo-free","providerID":"opencode","variant":"default"}}');
INSERT INTO session_message VALUES('msg_1211a8b9f001Q6Zi14rERCCLP5','ses_edee57470ffeJ8L7m282BCBa0v','user',5,1791556750331,1791556750332,'{"time":{"created":1791556750331},"text":"Reply with the single word: pong","files":[]}');
INSERT INTO session_message VALUES('msg_1211a8c08001oEYoSUv7Ta8qVA','ses_edee57470ffeJ8L7m282BCBa0v','assistant',6,1791556751940,1791556751949,'{"time":{"created":1791556751940,"streamed":1791556751948,"completed":1791556751949},"agent":"build","model":{"id":"big-pickle","providerID":"opencode","variant":"default"},"content":[{"type":"text","text":"pong"}],"finish":"stop","rawFinish":"stop","cost":0,"tokens":{"input":6363,"output":3,"reasoning":0,"cache":{"read":0,"write":0}}}');
