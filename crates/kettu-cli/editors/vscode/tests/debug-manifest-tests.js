const fs = require('fs');
const path = require('path');
const assert = require('assert');

const packageJsonPath = path.join(__dirname, '..', 'package.json');
const pkg = JSON.parse(fs.readFileSync(packageJsonPath, 'utf8'));

assert(pkg.contributes, 'package.json should define contributes');

const breakpoints = pkg.contributes.breakpoints;
assert(Array.isArray(breakpoints), 'contributes.breakpoints must be an array');
assert(
  breakpoints.some((bp) => bp && bp.language === 'kettu'),
  'breakpoints must include language "kettu"'
);

const debuggers = pkg.contributes.debuggers;
assert(Array.isArray(debuggers), 'contributes.debuggers must be an array');

const kettuDebugger = debuggers.find((dbg) => dbg && dbg.type === 'kettu');
assert(kettuDebugger, 'debuggers must include type "kettu"');
assert(
  Array.isArray(kettuDebugger.languages) && kettuDebugger.languages.includes('kettu'),
  'kettu debugger must support language "kettu"'
);

assert(
  Array.isArray(pkg.activationEvents) && pkg.activationEvents.includes('onDebugResolve:kettu'),
  'activationEvents must include onDebugResolve:kettu'
);

assert(
  pkg.activationEvents.includes('onCommand:kettu.debugCurrentFileTests'),
  'activationEvents must include onCommand:kettu.debugCurrentFileTests'
);

const commands = pkg.contributes.commands;
assert(Array.isArray(commands), 'contributes.commands must be an array');
assert(
  commands.some((cmd) => cmd && cmd.command === 'kettu.debugCurrentFileTests'),
  'commands must include kettu.debugCurrentFileTests'
);

console.log('Debug manifest tests passed');
