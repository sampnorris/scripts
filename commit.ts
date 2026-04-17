#!/usr/bin/env bun
/**
 * AI-powered conventional commit.
 * Usage: bun ~/.scripts/commit.ts [--yes] [--model <name>]
 *   - Stages nothing automatically; run `git add` first.
 *   - Generates a conventional commit message from the staged diff via local Ollama.
 */

import { $ } from "bun";

const MODEL = getArg("--model") ?? "gemma4:e2b";
const AUTO_YES = process.argv.includes("--yes") || process.argv.includes("-y");

function getArg(flag: string): string | undefined {
	const i = process.argv.indexOf(flag);
	return i >= 0 ? process.argv[i + 1] : undefined;
}

// Ensure we're in a git repo
try {
	await $`git rev-parse --is-inside-work-tree`.quiet();
} catch {
	console.error("Not inside a git repository.");
	process.exit(1);
}

// Stage all changes (tracked + untracked)
await $`git add -A`.quiet();

const diff = (await $`git diff --staged`.text()).trim();
if (!diff) {
	console.error("No changes to commit.");
	process.exit(1);
}

const status = (await $`git diff --staged --name-status`.text()).trim();

const system = `You write Conventional Commit messages.
Rules:
- Format: <type>(<optional scope>): <subject>
- Types: feat, fix, docs, style, refactor, perf, test, build, ci, chore, revert
- Subject: imperative mood, lowercase, no trailing period, <= 72 chars
- Optionally add a blank line then a short body explaining the "why" (wrap at 72)
- Output ONLY the commit message. No markdown, no code fences, no commentary.`;

const user = `Files changed:
${status}

Diff:
${diff.length > 12000 ? diff.slice(0, 12000) + "\n...[truncated]" : diff}

Write the commit message.`;

process.stderr.write(`Generating commit message with ${MODEL}...\n\n`);

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
let thinking = "";
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
			done?: boolean;
		};
		const t = chunk.message?.thinking;
		const c = chunk.message?.content;
		if (t) {
			if (mode !== "think") {
				process.stdout.write(`${DIM}💭 `);
				mode = "think";
			}
			process.stdout.write(`${DIM}${t}${RESET}`);
			thinking += t;
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

const message = content
	.trim()
	.replace(/^```[\w]*\n?/, "")
	.replace(/\n?```$/, "")
	.trim();

if (!message) {
	console.error("Model returned empty message.");
	process.exit(1);
}

if (!AUTO_YES) {
	process.stdout.write("Commit with this message? [Y/n/e(dit)] ");
	const answer = (await readLine()).trim().toLowerCase();
	if (answer === "n" || answer === "no") {
		console.log("Aborted.");
		process.exit(0);
	}
	if (answer === "e" || answer === "edit") {
		await $`git commit -e -m ${message}`;
		process.exit(0);
	}
}

await $`git commit -m ${message}`;

async function readLine(): Promise<string> {
	return new Promise((resolve) => {
		process.stdin.once("data", (d) => resolve(d.toString()));
	});
}
