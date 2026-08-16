'use strict';

// Host-side shutdown hook for NativeZsxSession.
//
// Native shutdown already shares a 500ms core budget. This Promise.race
// ceiling (800ms) is the Node consumer boundary: a stalled
// control/settlement/join cannot keep a host listener pending past the
// 2000ms exit deadline. The native hook applies the same ceiling even
// when this helper is not used.

const HOST_SHUTDOWN_HOOK_CEILING_MS = 800;
const HOST_EXIT_DEADLINE_MS = 2000;

function shutdownWithHostCeiling(session) {
  const snapshot = typeof session.status === 'function' ? session.status() : {};
  const generation = snapshot.generation;
  let timer;
  const timeout = new Promise(function (resolve) {
    timer = setTimeout(function () {
      resolve({
        kind: 'shutdown',
        generation: generation,
        reason: 'host_shutdown_timeout'
      });
    }, HOST_SHUTDOWN_HOOK_CEILING_MS);
  });
  return Promise.race([
    Promise.resolve(session.shutdown()).finally(function () {
      clearTimeout(timer);
    }),
    timeout
  ]);
}

module.exports = {
  HOST_SHUTDOWN_HOOK_CEILING_MS,
  HOST_EXIT_DEADLINE_MS,
  shutdownWithHostCeiling
};
