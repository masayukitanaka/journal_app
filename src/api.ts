import { invoke } from "@tauri-apps/api/core";
import type { PaperDetail, PaperSummary, SeedGraph } from "./types";

// 分野・年代の切り替え（旧 worker/db.ts の DB_REGISTRY に対応。現在は src-tauri/src/db.rs）。
export interface FieldInfo {
	field: number;
	name: string;
	yearRanges: string[]; // "2016-2026" 形式
	defaultYearRange: string | null; // yearRange 未指定時に実際に使われるレンジ
}

// invoke() はタイムアウトを持たないため、呼び出し側（App.tsx の SEED_FETCH_TIMEOUT_MS）が
// 期待する AbortError 相当の挙動を Promise.race で再現する。
function withTimeout<T>(promise: Promise<T>, timeoutMs?: number): Promise<T> {
	if (timeoutMs === undefined) return promise;
	return new Promise<T>((resolve, reject) => {
		const timer = setTimeout(() => reject(new DOMException("invoke timed out", "AbortError")), timeoutMs);
		promise.then(
			(value) => {
				clearTimeout(timer);
				resolve(value);
			},
			(err) => {
				clearTimeout(timer);
				reject(err);
			},
		);
	});
}

export function fetchFields(): Promise<{ fields: FieldInfo[] }> {
	return invoke<FieldInfo[]>("list_fields_cmd").then((fields) => ({ fields }));
}

export function fetchTop(field: number, yearRange: string | undefined, limit = 10): Promise<{ papers: PaperSummary[] }> {
	return invoke<PaperSummary[]>("fetch_top", { field, yearRange, limit }).then((papers) => ({ papers }));
}

export function fetchHubs(
	field: number,
	yearRange: string | undefined,
	minChild = 4,
	limit = 10,
): Promise<{ papers: PaperSummary[] }> {
	return invoke<PaperSummary[]>("fetch_hubs", { field, yearRange, minChild, limit }).then((papers) => ({ papers }));
}

export function fetchSeedGraph(
	field: number,
	yearRange: string | undefined,
	seedId: number,
	depth = 2,
	minCit = 0,
	timeoutMs?: number,
): Promise<SeedGraph> {
	return withTimeout(invoke<SeedGraph>("fetch_seed_graph", { field, yearRange, seedId, depth, minCit }), timeoutMs);
}

export function searchPapers(
	field: number,
	yearRange: string | undefined,
	q: string,
	limit = 20,
): Promise<{ papers: PaperSummary[] }> {
	return invoke<PaperSummary[]>("search_papers", { field, yearRange, q, limit }).then((papers) => ({ papers }));
}

export function fetchPaper(field: number, yearRange: string | undefined, id: number): Promise<PaperDetail> {
	return invoke<PaperDetail>("fetch_paper", { field, yearRange, id });
}

export interface AttributionSource {
	name: string;
	url: string;
	license: string;
	note: string;
}

export function fetchAttribution(): Promise<{ sources: AttributionSource[] }> {
	return invoke<AttributionSource[]>("fetch_attribution").then((sources) => ({ sources }));
}
