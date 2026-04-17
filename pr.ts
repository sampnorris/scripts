#!/usr/bin/env bun
/**
 * AI-powered pull request opener.
 * Usage: bun ~/.scripts/pr.ts [--base <branch>] [--model <name>] [--draft] [--yes]
 *   - Summarises commits + diff vs base branch via local Ollama
 *   - Pushes current branch and opens a PR via gh
 */

import { $ } from "bun";

const MODEL = getArg("--model") ?? "gemma4:e2b";
const BASE = getArg("--base") ?? (await detectBase());
const DRAFT = process.argv.includes("--draft");
const AUTO_YES = process.argv.includes("--yes") || process.argv.includes("-y");

function getArg(flag: string): string | undefined {
	const i = process.argv.indexOf(flag);
	return i >= 0 ? process.argv[i + 1] : undefined;
}

async function detectBase(): Promise<string> {
	try {
		const head = (await $`git symbolic-ref refs/remotes/origin/HEAD`.text()).trim();
		return head.replace("refs/remotes/origin/", "");
	} catch {
		for (const b of ["main", "master"]) {
			try {
				await $`git show-ref --verify --quiet refs/remotes/origin/${b}`.quiet();
				return b;
			} catch {}
		}
		return "main";
	}
}

// Must be in a git repo
try {
	await $`git rev-parse --is-inside-work-tree`.quiet();
} catch {
	console.error("Not inside a git repository.");
	process.exit(1);
}

const branch = (await $`git rev-parse --abbrev-ref HEAD`.text()).trim();
if (branch === BASE) {
	console.error(`On base branch '${BASE}'. Checkout a feature branch first.`);
	process.exit(1);
}

// Make sure base is up to date
process.stderr.write(`Fetching origin/${BASE}...\n`);
await $`git fetch origin ${BASE}`.quiet();

const mergeBase = (await $`git merge-base HEAD origin/${BASE}`.text()).trim();
const commits = (await $`git log --pretty=format:"- %s" ${mergeBase}..HEAD`.text()).trim();
const diffStat = (await $`git diff --stat ${mergeBase}..HEAD`.text()).trim();
const diff = (await $`git diff ${mergeBase}..HEAD`.text()).trim();

if (!commits) {
	console.error(`No commits on '${branch}' ahead of origin/${BASE}.`);
	process.exit(1);
}

const system = `You write concise, helpful GitHub pull request descriptions.
Output format (markdown):
  <short imperative title on a single line, <= 72 chars, no trailing period>
  <blank line>
  ## Summary
  - bullet points of what changed and why
  ## Changes
  - key file/area level changes
  ## Notes
  - optional: testing, risks, follow-ups (omit section if nothing to say)

Rules:
- Title first line only. No "#", no quotes, no prefix like "PR:".
- Then blank line, then body in markdown.
- No code fences around the whole thing. No commentary.`;

const truncatedDiff = diff.length > 16000 ? diff.slice(0, 16000) + "\n...[truncated]" : diff;

const user = `Branch: ${branch} -> ${BASE}

Commits:
${commits}

Diff stat:
${diffStat}

Diff:
${truncatedDiff}

Write the PR title and body.`;

process.stderr.write(`Generating PR description with ${MODEL}...\n\n`);

const res = await fetch("http://localhost:11434/api/chat", {
	method: "POST",
	body: JSON.stringify({
		model: MODEL,
		stream: true,
		think: true,
		messages: [
			{ role: "system", content: system },
			{ role: "user", content: user },
		],
		options: { temperature: 0.2 },
	}),
});

if (!res.ok || !res.body) {
	console.error(`Ollama error: ${res.status} ${await res.text()}`);
	process.exit(1);
}

const DIM = "\x1b[2m";
const RESET = "\x1b[0m";
const BOLD = "\x1b[1m";

let content = "";
let mode: "none" | "think" | "answer" = "none";

const reader = res.body.getReader();
const decoder = new TextDecoder();
let buf = "";

while (true) {
	const { done, value } = await reader.read();
	if (done) break;
	buf += decoder.decode(value, { stream: true });
	const lines = buf.split("\n");
	buf = lines.pop() ?? "";
	for (const line of lines) {
		if (!line.trim()) continue;
		const chunk = JSON.parse(line) as {
			message?: { content?: string; thinking?: string };
		};
		const t = chunk.message?.thinking;
		const c = chunk.message?.content;
		if (t) {
			if (mode !== "think") {
				process.stdout.write(`${DIM}💭 `);
				mode = "think";
			}
			process.stdout.write(`${DIM}${t}${RESET}`);
		}
		if (c) {
			if (mode !== "answer") {
				if (mode === "think") process.stdout.write(`${RESET}\n\n`);
				process.stdout.write(`${BOLD}`);
				mode = "answer";
			}
			process.stdout.write(c);
			content += c;
		}
	}
}
if (mode !== "none") process.stdout.write(`${RESET}\n`);

const cleaned = content
	.trim()
	.replace(/^```[\w]*\n?/, "")
	.replace(/\n?```$/, "")
	.trim();

const [titleLine = "", ...bodyLines] = cleaned.split("\n");
let title = titleLine.replace(/^#+\s*/, "").trim();
let body = bodyLines.join("\n").trim();

if (!title) {
	console.error("Model returned empty title.");
	process.exit(1);
}

console.log("\n--- Title ---");
console.log(title);
console.log("--- Body ---");
console.log(body);
console.log("-------------\n");

if (!AUTO_YES) {
	process.stdout.write("Open PR with this? [Y/n/e(dit)] ");
	const answer = (await readLine()).trim().toLowerCase();
	if (answer === "n" || answer === "no") {
		console.log("Aborted.");
		process.exit(0);
	}
	if (answer === "e" || answer === "edit") {
		const tmp = `/tmp/pr-${Date.now()}.md`;
		await Bun.write(tmp, `${title}\n\n${body}\n`);
		await $`${process.env.EDITOR ?? "vi"} ${tmp}`;
		const edited = await Bun.file(tmp).text();
		const [newTitle = "", ...rest] = edited.split("\n");
		title = newTitle.trim();
		body = rest.join("\n").trim();
	}
}

await openPR(title, body);

async function openPR(title: string, body: string) {
	process.stderr.write(`Pushing ${branch} to origin...\n`);
	await $`git push -u origin ${branch}`;

	const args = ["pr", "create", "--base", BASE, "--head", branch, "--title", title, "--body", body];
	if (DRAFT) args.push("--draft");
	await $`gh ${args}`;
}

async function readLine(): Promise<string> {
	return new Promise((resolve) => {
		process.stdin.once("data", (d) => resolve(d.toString()));
	});
}
