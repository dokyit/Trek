// The computer's own screens, as the iPhone's Basecamp tab, its Usage, its Search and a machine's
// Trek settings have them: Basecamp, plan usage, search over every thread, the computer's settings
// and its permission rules, each in the thread list's menu (where the computer serves it) and in
// a section of Settings. `machine_info` says what the computer runs on (its settings' line, its
// theme's "Match Windows"); `clone_project` and `put_files` are for one Trek driving another,
// which a phone doesn't do. The approval card's "Always allow…" is `AlwaysAllowSheet`.

import * as app from '../model.js';
import { registerListMenuItems, registerScreen, registerSettingsSection, registerSheet } from '../routes.js';
import { BasecampScreen } from './mac-basecamp.js';
import { UsageScreen } from './mac-usage.js';
import { SearchScreen } from './mac-search.js';
import { ComputerSection, ComputerSettingsScreen } from './mac-settings.js';
import { AlwaysAllowSheet, RulesScreen } from './mac-rules.js';

registerScreen('basecamp', BasecampScreen);
registerScreen('usage', UsageScreen);
registerScreen('search', SearchScreen);
registerScreen('computer-settings', ComputerSettingsScreen);
registerScreen('permission-rules', RulesScreen);
registerSheet('always-allow', AlwaysAllowSheet);

registerListMenuItems(({ has }) => [
  { label: 'Basecamp', icon: 'mountain', onClick: () => app.go({ screen: 'basecamp' }) },
  { label: 'Usage', icon: 'gauge', onClick: () => app.go({ screen: 'usage' }) },
  { label: 'Search', icon: 'search', hidden: !has('search'), onClick: () => app.go({ screen: 'search' }) },
  { label: 'Computer settings', icon: 'mac-monitor-cog', onClick: () => app.go({ screen: 'computer-settings' }) },
  { label: 'Permission rules', icon: 'shield', hidden: !has('rules'), onClick: () => app.go({ screen: 'permission-rules' }) },
]);

registerSettingsSection(ComputerSection);

export { AlwaysAllowSheet };
