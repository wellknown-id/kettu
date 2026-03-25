const fs = require("fs");
const path = require("path");
const assert = require("assert");

function readGrammar(fileName) {
  const filePath = path.join(__dirname, "..", "syntaxes", fileName);
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function getRepositoryPattern(grammar, repoKey) {
  const repositoryEntry = grammar.repository[repoKey];
  assert(repositoryEntry, `Repository entry ${repoKey} not found`);

  const [entry] = repositoryEntry.patterns;
  assert(entry && entry.match, `Pattern ${repoKey} is missing a match expression`);
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

  const keywords = getRepositoryPattern(grammar, "keywords");
  const operators = getRepositoryPattern(grammar, "operators");

  assertMatchesExactly(keywords, "async func", ["async", "func"]);
  assertMatchesExactly(keywords, "package world", ["package", "world"]);
  assertMatchesExactly(operators, "->", ["->"]);
  assertMatchesExactly(operators, "a - b", ["-"]);
  assertMatchesExactly(operators, "a>b", [">"]);
}

runFor("kettu.tmLanguage.json", "kettu");
runFor("wit.tmLanguage.json", "wit");

console.log("TextMate regex tests passed");
