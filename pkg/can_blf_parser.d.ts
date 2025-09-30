/* tslint:disable */
/* eslint-disable */
export function count_frames(blf_bytes: Uint8Array): any;
export class BlfSession {
  free(): void;
  [Symbol.dispose](): void;
  constructor(blf_bytes: Uint8Array, dbc_texts: any, channel_map: any);
  stats(): any;
  preview(n: number): any;
  signals(): any;
  decimated(max_points: number, keep_signals: any): any;
  export_csv(applied_signals: any): Uint8Array;
  free_memory(): void;
  static load_preview_smart(blf_bytes: Uint8Array, dbc_texts: any, channel_map: any, file_size: bigint): any;
  static export_csv_stream(blf_bytes: Uint8Array, dbc_texts: any, channel_map: any, progress_cb: Function): Uint8Array;
  static decimated_stream(blf_bytes: Uint8Array, dbc_texts: any, channel_map: any, max_points: number, progress_cb: Function): any;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
  readonly memory: WebAssembly.Memory;
  readonly __wbg_blfsession_free: (a: number, b: number) => void;
  readonly blfsession_new: (a: number, b: number, c: number, d: number, e: number) => void;
  readonly blfsession_stats: (a: number, b: number) => void;
  readonly blfsession_preview: (a: number, b: number, c: number) => void;
  readonly blfsession_signals: (a: number, b: number) => void;
  readonly blfsession_decimated: (a: number, b: number, c: number, d: number) => void;
  readonly blfsession_export_csv: (a: number, b: number, c: number) => void;
  readonly blfsession_free_memory: (a: number) => void;
  readonly blfsession_load_preview_smart: (a: number, b: number, c: number, d: number, e: number, f: bigint) => void;
  readonly blfsession_export_csv_stream: (a: number, b: number, c: number, d: number, e: number, f: number) => void;
  readonly blfsession_decimated_stream: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => void;
  readonly count_frames: (a: number, b: number, c: number) => void;
  readonly __wbindgen_export_0: (a: number) => void;
  readonly __wbindgen_export_1: (a: number, b: number) => number;
  readonly __wbindgen_export_2: (a: number, b: number, c: number, d: number) => number;
  readonly __wbindgen_add_to_stack_pointer: (a: number) => number;
  readonly __wbindgen_export_3: (a: number, b: number, c: number) => void;
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
