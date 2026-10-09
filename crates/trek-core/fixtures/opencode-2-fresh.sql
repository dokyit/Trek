-- A database made by OpenCode 2 (preview 0.0.0-beta-19296) with one session, opened over its API.
PRAGMA foreign_keys=OFF;
BEGIN TRANSACTION;
CREATE TABLE `account_state` (
          `id` integer PRIMARY KEY,
          `active_account_id` text,
          `active_org_id` text,
          CONSTRAINT `fk_account_state_active_account_id_account_id_fk` FOREIGN KEY (`active_account_id`) REFERENCES `account`(`id`) ON DELETE SET NULL
        );
CREATE TABLE `account` (
          `id` text PRIMARY KEY,
          `email` text NOT NULL,
          `url` text NOT NULL,
          `access_token` text NOT NULL,
          `refresh_token` text NOT NULL,
          `token_expiry` integer,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL
        );
CREATE TABLE `control_account` (
          `email` text NOT NULL,
          `url` text NOT NULL,
          `access_token` text NOT NULL,
          `refresh_token` text NOT NULL,
          `token_expiry` integer,
          `active` integer NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          CONSTRAINT `control_account_pk` PRIMARY KEY(`email`, `url`)
        );
CREATE TABLE `credential` (
          `id` text PRIMARY KEY,
          `integration_id` text,
          `label` text NOT NULL,
          `value` text NOT NULL,
          `connector_id` text,
          `method_id` text,
          `active` integer,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL
        );
CREATE TABLE `event_sequence` (
          `aggregate_id` text PRIMARY KEY,
          `seq` integer NOT NULL,
          `owner_id` text
        );
INSERT INTO event_sequence VALUES('ses_ede7c1be5ffePy3gmdS6lug9B9',0,NULL);
CREATE TABLE `event` (
          `id` text PRIMARY KEY,
          `aggregate_id` text NOT NULL,
          `seq` integer NOT NULL,
          `created` integer DEFAULT 0 NOT NULL,
          `type` text NOT NULL,
          `data` text NOT NULL,
          CONSTRAINT `fk_event_aggregate_id_event_sequence_aggregate_id_fk` FOREIGN KEY (`aggregate_id`) REFERENCES `event_sequence`(`aggregate_id`) ON DELETE CASCADE
        );
CREATE TABLE `kv` (
          `key` text PRIMARY KEY,
          `value` text NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL
        );
CREATE TABLE `permission` (
          `id` text PRIMARY KEY,
          `project_id` text NOT NULL,
          `action` text NOT NULL,
          `resource` text NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          CONSTRAINT `fk_permission_project_id_project_id_fk` FOREIGN KEY (`project_id`) REFERENCES `project`(`id`) ON DELETE CASCADE
        );
CREATE TABLE `project_directory` (
          `project_id` text NOT NULL,
          `directory` text NOT NULL,
          `type` text,
          `strategy` text,
          `time_created` integer NOT NULL,
          CONSTRAINT `project_directory_pk` PRIMARY KEY(`project_id`, `directory`),
          CONSTRAINT `fk_project_directory_project_id_project_id_fk` FOREIGN KEY (`project_id`) REFERENCES `project`(`id`) ON DELETE CASCADE
        );
CREATE TABLE `project` (
          `id` text PRIMARY KEY,
          `worktree` text NOT NULL,
          `vcs` text,
          `name` text,
          `icon_url` text,
          `icon_url_override` text,
          `icon_color` text,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `time_initialized` integer,
          `sandboxes` text NOT NULL,
          `commands` text
        );
INSERT INTO project VALUES('70b4fd70dc4b0d210c318e4a217f514d8427ab96','/Users/me/app',NULL,NULL,NULL,NULL,NULL,1791563494856,1791563494856,NULL,'[]',NULL);
CREATE TABLE `instruction_blob` (
          `hash` text PRIMARY KEY,
          `value` text
        );
CREATE TABLE `instruction_entry` (
          `session_id` text NOT NULL,
          `key` text NOT NULL,
          `value` text,
          `removed` integer DEFAULT false NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          CONSTRAINT `instruction_entry_pk` PRIMARY KEY(`session_id`, `key`),
          CONSTRAINT `fk_instruction_entry_session_id_session_v2_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session_v2`(`id`) ON DELETE CASCADE
        );
CREATE TABLE `instruction_state` (
          `session_id` text PRIMARY KEY,
          `epoch_start` integer NOT NULL,
          `through_seq` integer NOT NULL,
          `initial_values` text NOT NULL,
          `current_values` text NOT NULL,
          CONSTRAINT `fk_instruction_state_session_id_session_v2_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session_v2`(`id`) ON DELETE CASCADE
        );
CREATE TABLE `session_inbox` (
          `id` text PRIMARY KEY,
          `session_id` text NOT NULL,
          `type` text NOT NULL,
          `payload` text NOT NULL,
          `delivery` text NOT NULL,
          `enqueued_seq` integer NOT NULL,
          `time_created` integer NOT NULL,
          CONSTRAINT `fk_session_inbox_session_id_session_v2_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session_v2`(`id`) ON DELETE CASCADE
        );
CREATE TABLE `session_message` (
          `id` text PRIMARY KEY,
          `session_id` text NOT NULL,
          `type` text NOT NULL,
          `seq` integer NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `data` text NOT NULL,
          CONSTRAINT `fk_session_message_session_id_session_v2_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session_v2`(`id`) ON DELETE CASCADE
        );
CREATE TABLE `session_pending` (
          `id` text PRIMARY KEY,
          `session_id` text NOT NULL,
          `type` text NOT NULL,
          `data` text NOT NULL,
          `delivery` text,
          `admitted_seq` integer NOT NULL,
          `time_created` integer NOT NULL,
          CONSTRAINT `fk_session_pending_session_id_session_v2_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session_v2`(`id`) ON DELETE CASCADE
        );
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
          `time_idle` integer,
          `time_viewed` integer,
          `idle_outcome` text,
          `time_compacting` integer,
          `time_archived` integer,
          `time_suspended` integer,
          `resume_attempts` integer DEFAULT 0 NOT NULL,
          CONSTRAINT `fk_session_v2_project_id_project_id_fk` FOREIGN KEY (`project_id`) REFERENCES `project`(`id`) ON DELETE CASCADE
        );
INSERT INTO session_v2 VALUES('ses_ede7c1be5ffePy3gmdS6lug9B9','70b4fd70dc4b0d210c318e4a217f514d8427ab96',NULL,NULL,NULL,NULL,'playful-garden','/Users/me/app','','2.0 session one','0.0.0-beta-19296',NULL,NULL,NULL,NULL,NULL,NULL,0.0,0,0,0,0,0,NULL,NULL,NULL,NULL,1791563654176,1791563654176,NULL,NULL,NULL,NULL,NULL,NULL,0);
CREATE TABLE `workspace` (
          `id` text PRIMARY KEY,
          `provider` text NOT NULL,
          `binding` text,
          `created_at` integer NOT NULL,
          `last_used_at` integer NOT NULL
        );
CREATE TABLE `worktree` (
          `project_id` text NOT NULL,
          `directory` text NOT NULL,
          `strategy` text,
          `time_created` integer NOT NULL,
          CONSTRAINT `worktree_pk` PRIMARY KEY(`project_id`, `directory`),
          CONSTRAINT `fk_worktree_project_id_project_id_fk` FOREIGN KEY (`project_id`) REFERENCES `project`(`id`) ON DELETE CASCADE
        );
CREATE TABLE IF NOT EXISTS "migration" (id TEXT PRIMARY KEY, time_completed INTEGER NOT NULL);
INSERT INTO migration VALUES('20260127222353_familiar_lady_ursula',1791563494805);
INSERT INTO migration VALUES('20260211171708_add_project_commands',1791563494805);
INSERT INTO migration VALUES('20260213144116_wakeful_the_professor',1791563494805);
INSERT INTO migration VALUES('20260225215848_workspace',1791563494805);
INSERT INTO migration VALUES('20260227213759_add_session_workspace_id',1791563494805);
INSERT INTO migration VALUES('20260228203230_blue_harpoon',1791563494805);
INSERT INTO migration VALUES('20260303231226_add_workspace_fields',1791563494805);
INSERT INTO migration VALUES('20260309230000_move_org_to_state',1791563494806);
INSERT INTO migration VALUES('20260312043431_session_message_cursor',1791563494806);
INSERT INTO migration VALUES('20260323234822_events',1791563494806);
INSERT INTO migration VALUES('20260410174513_workspace-name',1791563494806);
INSERT INTO migration VALUES('20260413175956_chief_energizer',1791563494806);
INSERT INTO migration VALUES('20260423070820_add_icon_url_override',1791563494806);
INSERT INTO migration VALUES('20260427172553_slow_nightmare',1791563494806);
INSERT INTO migration VALUES('20260428004200_add_session_path',1791563494806);
INSERT INTO migration VALUES('20260501142318_next_venus',1791563494806);
INSERT INTO migration VALUES('20260504145000_add_sync_owner',1791563494806);
INSERT INTO migration VALUES('20260507164347_add_workspace_time',1791563494806);
INSERT INTO migration VALUES('20260510033149_session_usage',1791563494806);
INSERT INTO migration VALUES('20260511000411_data_migration_state',1791563494806);
INSERT INTO migration VALUES('20260511173437_session-metadata',1791563494806);
INSERT INTO migration VALUES('20260601010001_normalize_storage_paths',1791563494806);
INSERT INTO migration VALUES('20260601202201_amazing_prowler',1791563494806);
INSERT INTO migration VALUES('20260602002951_lowly_union_jack',1791563494806);
INSERT INTO migration VALUES('20260602182828_add_project_directories',1791563494806);
INSERT INTO migration VALUES('20260603001617_session_message_projection_indexes',1791563494806);
INSERT INTO migration VALUES('20260603040000_session_message_projection_order',1791563494806);
INSERT INTO migration VALUES('20260603141458_session_input_inbox',1791563494806);
INSERT INTO migration VALUES('20260603160727_jittery_ezekiel_stane',1791563494806);
INSERT INTO migration VALUES('20260604172448_event_sourced_session_input',1791563494806);
INSERT INTO migration VALUES('20260605003541_add_session_context_snapshot',1791563494806);
INSERT INTO migration VALUES('20260605042240_add_context_epoch_agent',1791563494806);
INSERT INTO migration VALUES('20260611035744_credential',1791563494806);
INSERT INTO migration VALUES('20260611192811_lush_chimera',1791563494806);
INSERT INTO migration VALUES('20260612174303_project_dir_strategy',1791563494806);
INSERT INTO migration VALUES('20260622142730_simplify_session_context_epoch',1791563494806);
INSERT INTO migration VALUES('20260622170816_reset_v2_session_state',1791563494807);
INSERT INTO migration VALUES('20260622202450_simplify_session_input',1791563494807);
INSERT INTO migration VALUES('20260804233008_loose_psylocke',1791563494807);
INSERT INTO migration VALUES('20260805200742_import_legacy_credentials',1791563494807);
INSERT INTO migration VALUES('20260808023530_workspace_domain',1791563494807);
INSERT INTO migration VALUES('20260811161259_execution_claim_attempts',1791563494807);
INSERT INTO migration VALUES('20260812181746_session_inbox',1791563494807);
INSERT INTO migration VALUES('20260812213948_worktree',1791563494807);
INSERT INTO migration VALUES('20260819222447_session_viewed_state',1791563494807);
INSERT INTO migration VALUES('20260823191254_nullable_workspace_binding',1791563494807);
CREATE UNIQUE INDEX `event_aggregate_seq_idx` ON `event` (`aggregate_id`,`seq`);
CREATE INDEX `event_aggregate_type_seq_idx` ON `event` (`aggregate_id`,`type`,`seq`);
CREATE UNIQUE INDEX `permission_project_action_resource_idx` ON `permission` (`project_id`,`action`,`resource`);
CREATE INDEX `session_inbox_session_delivery_seq_idx` ON `session_inbox` (`session_id`,`delivery`,`enqueued_seq`);
CREATE UNIQUE INDEX `session_inbox_session_enqueued_seq_idx` ON `session_inbox` (`session_id`,`enqueued_seq`);
CREATE UNIQUE INDEX `session_message_session_seq_idx` ON `session_message` (`session_id`,`seq`);
CREATE INDEX `session_message_session_type_seq_idx` ON `session_message` (`session_id`,`type`,`seq`);
CREATE INDEX `session_message_session_time_created_id_idx` ON `session_message` (`session_id`,`time_created`,`id`);
CREATE INDEX `session_message_time_created_idx` ON `session_message` (`time_created`);
CREATE INDEX `session_pending_session_delivery_seq_idx` ON `session_pending` (`session_id`,`delivery`,`admitted_seq`);
CREATE UNIQUE INDEX `session_pending_session_compaction_idx` ON `session_pending` (`session_id`) WHERE "session_pending"."type" = 'compaction';
CREATE UNIQUE INDEX `session_pending_session_admitted_seq_idx` ON `session_pending` (`session_id`,`admitted_seq`);
CREATE INDEX `session_v2_project_idx` ON `session_v2` (`project_id`);
CREATE INDEX `session_v2_workspace_idx` ON `session_v2` (`workspace_id`);
CREATE INDEX `session_v2_parent_idx` ON `session_v2` (`parent_id`);
CREATE INDEX `session_v2_time_suspended_idx` ON `session_v2` (`time_suspended`) WHERE "session_v2"."time_suspended" is not null;
COMMIT;
