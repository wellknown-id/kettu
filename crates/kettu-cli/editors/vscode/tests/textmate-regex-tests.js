const fs = require("fs");
const path = require("path");
const assert = require("assert");

function readGrammar(fileName) {
  const filePath = path.join(__dirname, "..", "syntaxes", fileName);
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function getRepositoryPattern(grammar, repoKey, scopeName) {
  const repo = grammar.repository[repoKey];
  assert(repo, `Repository key '${repoKey}' not found`);
  const entry = repo.patterns.find((p) => p.name === scopeName);
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

  // Verify comment patterns exist
  assert(grammar.repository.comments, "Grammar should have comments repository");
  const commentPatterns = grammar.repository.comments.patterns;
  assert(
    commentPatterns.some((p) => p.name === `comment.line.double-slash.${languageSuffix}`),
    "Should have line comment pattern"
  );
  assert(
    commentPatterns.some((p) => p.name === `comment.block.${languageSuffix}`),
    "Should have block comment pattern"
  );

  // Verify comments appear before keywords in top-level patterns
  const topPatterns = grammar.patterns.map((p) => p.include);
  const commentsIdx = topPatterns.indexOf("#comments");
  const keywordsIdx = topPatterns.indexOf("#keywords");
  assert(commentsIdx >= 0, "Comments should be in top-level patterns");
  assert(commentsIdx < keywordsIdx, "Comments should appear before keywords");

  // Verify keywords include 'async'
  const kwPattern = getRepositoryPattern(
    grammar,
    "keywords",
    `keyword.control.${languageSuffix}`
  );
  assertMatchesExactly(kwPattern, "async func() -> s32", ["async", "func"]);
  assertMatchesExactly(kwPattern, "dummy-async", ["async"]);

  // Verify operators include '->'
  const opPattern = getRepositoryPattern(
    grammar,
    "operators",
    `keyword.operator.${languageSuffix}`
  );
  assertMatchesExactly(opPattern, "->", ["->"]);

  // Verify line comment pattern matches
  const lineComment = new RegExp(
    commentPatterns.find((p) => p.match).match
  );
  assert(lineComment.test("// this is a comment"), "Should match line comments");
}

runFor("kettu.tmLanguage.json", "kettu");
runFor("wit.tmLanguage.json", "wit");

console.log("TextMate regex tests passed");
