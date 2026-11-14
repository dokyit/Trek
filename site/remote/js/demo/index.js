// The demo's answers to the requests each area adds (`demo/*.js`), merged: `type → (msg, demo) => reply`.
import notes from './notes.js';
import git from './git.js';
import mac from './mac.js';
import thread, { seed as threadSeed } from './thread.js';
import media from './media.js';

export const handlers = { ...notes, ...git, ...mac, ...thread, ...media };

/** What areas add to the demo's threads when it starts: `seed(demo)`. */
export const seeds = [threadSeed];
