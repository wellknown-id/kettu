const fs = require("fs");
const path = require("path");
const assert = require("assert");

function readGrammar(fileName) {
  const filePath = path.join(__dirname, "..", "syntaxes", fileName);
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function getRepositoryPattern(grammar, repoKey, scopeName) {
  const patterns = grammar.repository[repoKey].patterns;
  const entry = patterns.find((p) => p.name === scopeName);
  assert(entry, `Pattern ${scopeName} not found under ${repoKey}`);
  return new RegExp(entry.match, "g");
}

function assertMatchesExactly(regex, input, expected) {
  const matches = [...input.matchAll(regex)].map((m) => m[0]);
  assert.deepStrictEqual(
    matches,
    expected,
    `Expected matches ${JSON.stringify(expected)} but got ${JSON.stringify(matches)} for input: ${input}`
  );
}

function runFor(grammarFile, languageSuffix) {
  const grammar = readGrammar(grammarFile);

  const asyncKw = getRepositoryPattern(
    grammar,
    "function-declaration",
    `keyword.declaration.async.${languageSuffix}`
  );
  const arrowOp = getRepositoryPattern(
    grammar,
    "operators",
    `keyword.operator.arrow.${languageSuffix}`
  );
  const comparisonOp = getRepositoryPattern(
    grammar,
    "operators",
    `keyword.operator.comparison.${languageSuffix}`
  );
  const arithmeticOp = getRepositoryPattern(
    grammar,
    "operators",
    `keyword.operator.arithmetic.${languageSuffix}`
  );

  assertMatchesExactly(asyncKw, "dummy-async: async func() -> s32", ["async"]);
  assertMatchesExactly(asyncKw, "dummy-async: func() -> s32", []);

  assertMatchesExactly(arrowOp, "dummy-async: async func() -> s32", ["->"]);
  assertMatchesExactly(comparisonOp, "dummy-async: async func() -> s32", []);
  assertMatchesExactly(arithmeticOp, "dummy-async", []);
  assertMatchesExactly(arithmeticOp, "dummy - async", ["-"]);
  assertMatchesExactly(arithmeticOp, "a->b", []);
  assertMatchesExactly(comparisonOp, "a->b", []);
  assertMatchesExactly(comparisonOp, "a>b", [">"]);
}

runFor("kettu.tmLanguage.json", "kettu");
runFor("wit.tmLanguage.json", "wit");

console.log("TextMate regex tests passed");
