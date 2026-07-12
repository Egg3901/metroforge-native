#!/usr/bin/env bun
/**
 * Validate every scenario JSON file against scenario.schema.json, and the
 * progression manifest against progression.schema.json + catalog consistency.
 *
 * Usage: bun run validate-scenarios
 * Exit 0 on success; non-zero with a readable report on failure.
 */
import { readdir, readFile } from 'node:fs/promises';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import Ajv2020 from 'ajv/dist/2020.js';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const DIR = join(ROOT, 'src/content/scenarios');
const SCHEMA_PATH = join(DIR, 'scenario.schema.json');
const PROG_SCHEMA_PATH = join(DIR, 'progression.schema.json');
const PROG_PATH = join(DIR, 'progression.json');

const RESERVED = new Set(['scenario.schema.json', 'progression.schema.json', 'progression.json']);

interface Issue {
  file: string;
  message: string;
}

async function loadJson(path: string): Promise<unknown> {
  return JSON.parse(await readFile(path, 'utf8')) as unknown;
}

function dashFree(s: string): boolean {
  // em dash, en dash, horizontal bar — titles/descriptions must stay dash-free
  return !/[\u2012\u2013\u2014\u2015]/.test(s);
}

async function main(): Promise<void> {
  const issues: Issue[] = [];
  const ajv = new Ajv2020({ allErrors: true, strict: false });
  const scenarioSchema = await loadJson(SCHEMA_PATH);
  const progressionSchema = await loadJson(PROG_SCHEMA_PATH);
  const validateScenario = ajv.compile(scenarioSchema);
  const validateProgression = ajv.compile(progressionSchema);

  const files = (await readdir(DIR)).filter((f) => f.endsWith('.json') && !RESERVED.has(f)).sort();
  if (files.length === 0) {
    issues.push({ file: DIR, message: 'no scenario JSON files found' });
  }

  const ids = new Set<string>();
  const byId = new Map<string, { label: string; description: string; cityKey: string }>();

  for (const file of files) {
    const path = join(DIR, file);
    let data: Record<string, unknown>;
    try {
      data = (await loadJson(path)) as Record<string, unknown>;
    } catch (e) {
      issues.push({ file, message: `invalid JSON: ${(e as Error).message}` });
      continue;
    }

    if (!validateScenario(data)) {
      for (const err of validateScenario.errors ?? []) {
        issues.push({
          file,
          message: `${err.instancePath || '/'} ${err.message ?? 'schema error'}`,
        });
      }
    }

    const id = typeof data.id === 'string' ? data.id : '';
    const expected = file.replace(/\.json$/, '');
    if (id && id !== expected) {
      issues.push({ file, message: `id "${id}" must match filename stem "${expected}"` });
    }
    if (id) {
      if (ids.has(id)) issues.push({ file, message: `duplicate id "${id}"` });
      ids.add(id);
      byId.set(id, {
        label: String(data.label ?? ''),
        description: String(data.description ?? ''),
        cityKey: String(data.cityKey ?? ''),
      });
    }

    if (typeof data.label === 'string' && !dashFree(data.label)) {
      issues.push({ file, message: 'label contains an em/en dash (dash-free copy required)' });
    }
    if (typeof data.description === 'string' && !dashFree(data.description)) {
      issues.push({ file, message: 'description contains an em/en dash (dash-free copy required)' });
    }
    if (Array.isArray(data.events)) {
      for (const ev of data.events as { message?: string; id?: string }[]) {
        if (ev.message && !dashFree(ev.message)) {
          issues.push({ file, message: `event ${ev.id ?? '?'} message contains an em/en dash` });
        }
      }
    }
  }

  // progression
  let progression: { starters: string[]; unlocks: Record<string, string[]> };
  try {
    progression = (await loadJson(PROG_PATH)) as typeof progression;
  } catch (e) {
    issues.push({ file: 'progression.json', message: `invalid JSON: ${(e as Error).message}` });
    report(issues, files.length);
    process.exit(1);
  }

  if (!validateProgression(progression)) {
    for (const err of validateProgression.errors ?? []) {
      issues.push({
        file: 'progression.json',
        message: `${err.instancePath || '/'} ${err.message ?? 'schema error'}`,
      });
    }
  }

  for (const starter of progression.starters ?? []) {
    if (!ids.has(starter)) {
      issues.push({ file: 'progression.json', message: `starter "${starter}" is not a catalog scenario` });
    }
  }

  const unlockTargets = new Set<string>();
  for (const [from, tos] of Object.entries(progression.unlocks ?? {})) {
    if (!ids.has(from)) {
      issues.push({ file: 'progression.json', message: `unlock key "${from}" is not a catalog scenario` });
    }
    for (const to of tos) {
      if (!ids.has(to)) {
        issues.push({ file: 'progression.json', message: `unlock target "${to}" (from ${from}) is not a catalog scenario` });
      }
      unlockTargets.add(to);
      if (to === from) {
        issues.push({ file: 'progression.json', message: `self-unlock "${from}" → "${to}"` });
      }
    }
  }

  // every non-starter must be reachable via at least one unlock edge
  for (const id of ids) {
    if (progression.starters.includes(id)) continue;
    if (!unlockTargets.has(id)) {
      issues.push({
        file: 'progression.json',
        message: `scenario "${id}" is neither a starter nor unlocked by any edge`,
      });
    }
  }

  // cycle detection (DFS)
  const visiting = new Set<string>();
  const visited = new Set<string>();
  const cycleFrom = (node: string, stack: string[]): void => {
    if (visiting.has(node)) {
      issues.push({
        file: 'progression.json',
        message: `cycle detected: ${[...stack, node].join(' → ')}`,
      });
      return;
    }
    if (visited.has(node)) return;
    visiting.add(node);
    for (const next of progression.unlocks[node] ?? []) {
      cycleFrom(next, [...stack, node]);
    }
    visiting.delete(node);
    visited.add(node);
  };
  for (const id of ids) cycleFrom(id, []);

  // city mix sanity: catalog must include both Cleveland and NYC
  const cities = new Set([...byId.values()].map((s) => s.cityKey));
  if (!cities.has('cleveland') || !cities.has('nyc')) {
    issues.push({ file: DIR, message: 'catalog must include both cleveland and nyc scenarios' });
  }

  report(issues, files.length);
  process.exit(issues.length ? 1 : 0);
}

function report(issues: Issue[], count: number): void {
  if (issues.length === 0) {
    console.log(`validate-scenarios: ok (${count} scenarios + progression)`);
    return;
  }
  console.error(`validate-scenarios: ${issues.length} issue(s)`);
  for (const i of issues) console.error(`  · ${i.file}: ${i.message}`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
