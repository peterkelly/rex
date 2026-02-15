/* tslint:disable */
/* eslint-disable */

export function evalToJson(source: string, gas_limit?: bigint | null): string;

export function evalToString(source: string, gas_limit?: bigint | null): string;

export function inferToJson(source: string, gas_limit?: bigint | null): string;

export function lspCompletionsToJson(source: string, line: number, character: number): string;

export function lspDiagnosticsToJson(source: string): string;

export function lspDocumentSymbolsToJson(source: string): string;

export function lspFormatToJson(source: string): string;

export function lspGotoDefinitionToJson(source: string, line: number, character: number): string;

export function lspHoverToJson(source: string, line: number, character: number): string;

export function lspReferencesToJson(source: string, line: number, character: number, include_declaration: boolean): string;

export function lspRenameToJson(source: string, line: number, character: number, new_name: string): string;

export function parseToJson(source: string, gas_limit?: bigint | null): string;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
  readonly memory: WebAssembly.Memory;
  readonly evalToJson: (a: number, b: number, c: number, d: bigint) => [number, number, number, number];
  readonly evalToString: (a: number, b: number, c: number, d: bigint) => [number, number, number, number];
  readonly inferToJson: (a: number, b: number, c: number, d: bigint) => [number, number, number, number];
  readonly lspCompletionsToJson: (a: number, b: number, c: number, d: number) => [number, number, number, number];
  readonly lspDiagnosticsToJson: (a: number, b: number) => [number, number, number, number];
  readonly lspDocumentSymbolsToJson: (a: number, b: number) => [number, number, number, number];
  readonly lspFormatToJson: (a: number, b: number) => [number, number, number, number];
  readonly lspGotoDefinitionToJson: (a: number, b: number, c: number, d: number) => [number, number, number, number];
  readonly lspHoverToJson: (a: number, b: number, c: number, d: number) => [number, number, number, number];
  readonly lspReferencesToJson: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
  readonly lspRenameToJson: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number, number];
  readonly parseToJson: (a: number, b: number, c: number, d: bigint) => [number, number, number, number];
  readonly __wbindgen_exn_store: (a: number) => void;
  readonly __externref_table_alloc: () => number;
  readonly __wbindgen_externrefs: WebAssembly.Table;
  readonly __wbindgen_malloc: (a: number, b: number) => number;
  readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
  readonly __externref_table_dealloc: (a: number) => void;
  readonly __wbindgen_free: (a: number, b: number, c: number) => void;
  readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
* Instantiates the given `module`, which can either be bytes or
* a precompiled `WebAssembly.Module`.
*
* @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
*
* @returns {InitOutput}
*/
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
* If `module_or_path` is {RequestInfo} or {URL}, makes a request and
* for everything else, calls `WebAssembly.instantiate` directly.
*
* @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
*
* @returns {Promise<InitOutput>}
*/
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
