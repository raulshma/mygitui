<!--
  FileEditor — one open text file in the file workspace, backed by
  CodeMirror 6 (imported dynamically inside the mount effect, so the
  ~400KB editor chunk never loads until a text file actually opens).

  Interaction model:
  - Opens read-mode-fast: content is loaded over `repo_read_file` and the
    editor is immediately editable; every doc change flips `dirty`.
  - Ctrl+S (palette `files.save` → `files-save` event, plus an in-editor
    keymap for contexts without the global keybinds) saves through
    `repo_write_file` with the mtime guard; the first save of a dirty
    session snapshots a checkpoint (`edit-file <path>`) so the edit is
    undoable from the Undo panel.
  - "Changed on disk" (external modification while editing) surfaces a
    Reload / Keep-mine dialog; Keep-mine overwrites unconditionally.
  - The per-language grammar loads lazily via @codemirror/language-data;
    the editor theme is built from the app's theme CSS variables so it
    follows the color scheme automatically.
-->
<script lang="ts">
  import type { FileMeta } from "$lib/ipc/types";
  import { checkpointCreate, readFile, writeFile } from "$lib/ipc/client";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { toast } from "$lib/toast";
  import { tags } from "@lezer/highlight";
  import { editorPrefs } from "./editorPrefs.svelte";

  let {
    repoId,
    path,
    meta,
    dirty = $bindable(false),
    onSaved = undefined,
    previewMode = "off",
  }: {
    repoId: string;
    /** Workdir-relative path (forward slashes). */
    path: string;
    /** Fresh stat from open time (mtime guard anchor). */
    meta: FileMeta;
    dirty?: boolean;
    /** After a successful save (workspace refreshes status). */
    onSaved?: () => void;
    /** Markdown preview layout (parent decides; markdown files only). */
    previewMode?: "off" | "split" | "preview";
  } = $props();

  let status = $state<"loading" | "ready" | "error">("loading");
  let errorText = $state("");
  let host: HTMLDivElement | undefined = $state();

  /** Bumped on every doc change; drives the (debounced) markdown preview. */
  let docVersion = $state(0);

  const showPreview = $derived(previewMode !== "off" && status === "ready");
  /** In "preview" mode the editor column hides but CM stays mounted. */
  const editorHidden = $derived(previewMode === "preview");

  // Mutable editor plumbing (not reactive — the DOM is the source of truth).
  let view: unknown = null;
  /** Current file metadata (mtime updates after each save). */
  // svelte-ignore state_referenced_locally -- the mount-time stat is the
  // intended anchor; reload()/save() update this field explicitly.
  let currentMeta: FileMeta = meta;
  /** Checkpoint taken for the current dirty session (one per session). */
  let checkpointed = false;
  /** Guards the async load against rapid path changes. */
  let loadToken = 0;
  /** Live markdown preview instance (split/preview modes). */
  let previewRef = $state<PreviewApi | null>(null);
  /** rAF coalescing for editor-scroll → preview sync. */
  let scrollRaf = 0;

  interface PreviewApi {
    syncFromEditor(line: number, fallbackLineHeight: number): void;
  }

  const decoder = new TextDecoder();
  const encoder = new TextEncoder();

  // Dynamic CM modules (resolved once per component lifetime).
  type Cm = {
    EditorView: (typeof import("@codemirror/view"))["EditorView"];
    keymap: (typeof import("@codemirror/view"))["keymap"];
    basicSetup: import("@codemirror/state").Extension;
    lineWrapping: import("@codemirror/state").Extension;
    EditorState: typeof import("@codemirror/state")["EditorState"];
    Compartment: typeof import("@codemirror/state")["Compartment"];
    Prec: typeof import("@codemirror/state")["Prec"];
    HighlightStyle: typeof import("@codemirror/language")["HighlightStyle"];
    syntaxHighlighting: typeof import("@codemirror/language")["syntaxHighlighting"];
    LanguageDescription: typeof import("@codemirror/language")["LanguageDescription"];
    languages: typeof import("@codemirror/language-data")["languages"];
  };
  let cm: Cm | null = null;
  let wrapCompartment: import("@codemirror/state").Compartment | null = null;
  let languageCompartment: import("@codemirror/state").Compartment | null = null;

  function basename(path: string): string {
    const idx = path.lastIndexOf("/");
    return idx === -1 ? path : path.slice(idx + 1);
  }

  async function loadCm(): Promise<Cm> {
    const [core, state, viewMod, language, languageData] = await Promise.all([
      import("codemirror"),
      import("@codemirror/state"),
      import("@codemirror/view"),
      import("@codemirror/language"),
      import("@codemirror/language-data"),
    ]);
    return {
      EditorView: core.EditorView,
      keymap: viewMod.keymap,
      basicSetup: core.basicSetup,
      lineWrapping: core.EditorView.lineWrapping,
      EditorState: state.EditorState,
      Compartment: state.Compartment,
      Prec: state.Prec,
      HighlightStyle: language.HighlightStyle,
      syntaxHighlighting: language.syntaxHighlighting,
      LanguageDescription: language.LanguageDescription,
      languages: languageData.languages,
    };
  }

  /** App-themed editor + syntax theme (CSS variables → follows theme). */
  function buildTheme(c: Cm) {
    const editorTheme = c.EditorView.theme({
      "&": {
        color: "var(--foreground)",
        backgroundColor: "var(--card)",
        fontSize: "0.8125rem",
      },
      ".cm-content": {
        caretColor: "var(--primary)",
        fontFamily: "var(--app-font-mono, ui-monospace, Consolas, monospace)",
      },
      ".cm-scroller": {
        fontFamily: "var(--app-font-mono, ui-monospace, Consolas, monospace)",
        lineHeight: "1.5",
      },
      ".cm-gutters": {
        backgroundColor: "var(--muted)",
        color: "var(--muted-foreground)",
        border: "none",
        borderRight: "1px solid var(--border)",
      },
      ".cm-activeLine": {
        backgroundColor:
          "color-mix(in srgb, var(--primary) 7%, transparent)",
      },
      ".cm-activeLineGutter": {
        backgroundColor:
          "color-mix(in srgb, var(--primary) 12%, transparent)",
        color: "var(--foreground)",
      },
      "&.cm-focused .cm-selectionBackground, .cm-selectionBackground":
        {
          backgroundColor:
            "color-mix(in srgb, var(--primary) 22%, transparent) !important",
        },
      ".cm-selectionMatch": {
        backgroundColor:
          "color-mix(in srgb, var(--muted) 25%, transparent)",
      },
      ".cm-searchMatch": {
        backgroundColor:
          "color-mix(in srgb, var(--muted) 30%, transparent)",
        outline: "1px solid var(--muted)",
      },
      ".cm-searchMatch-selected": {
        backgroundColor:
          "color-mix(in srgb, var(--primary) 35%, transparent)",
      },
      ".cm-panels": {
        backgroundColor: "var(--muted)",
        color: "var(--foreground)",
        borderBottom: "1px solid var(--border)",
      },
      ".cm-panel.cm-search input, .cm-panel.cm-search button": {
        background: "var(--card)",
        color: "var(--foreground)",
        border: "1px solid var(--border)",
        borderRadius: "4px",
        font: "inherit",
        padding: "0.05rem 0.3rem",
      },
      ".cm-tooltip": {
        backgroundColor: "var(--accent)",
        color: "var(--foreground)",
        border: "1px solid var(--border)",
      },
    });
    const highlight = c.syntaxHighlighting(
      c.HighlightStyle.define([
        { tag: tags.keyword, color: "var(--primary)" },
        { tag: [tags.controlKeyword, tags.moduleKeyword], color: "var(--primary)" },
        { tag: tags.string, color: "var(--muted)" },
        { tag: tags.number, color: "var(--muted)" },
        {
          tag: tags.comment,
          color: "var(--muted-foreground)",
          fontStyle: "italic",
        },
        { tag: [tags.function(tags.variableName), tags.function(tags.propertyName)], color: "var(--secondary)" },
      { tag: [tags.typeName, tags.className], color: "var(--secondary)" },
      { tag: [tags.variableName, tags.propertyName], color: "var(--foreground)" },
      { tag: tags.heading, color: "var(--primary)", fontWeight: "bold" },
      { tag: tags.link, color: "var(--muted)", textDecoration: "underline" },
      { tag: tags.invalid, color: "var(--destructive)" },
      // Markdown source + general coverage polish.
      { tag: tags.strong, fontWeight: "600" },
      { tag: tags.emphasis, fontStyle: "italic" },
      { tag: tags.strikethrough, textDecoration: "line-through" },
      { tag: tags.quote, color: "var(--muted-foreground)", fontStyle: "italic" },
      { tag: tags.contentSeparator, color: "var(--primary)" },
      { tag: tags.monospace, fontFamily: "var(--app-font-mono, ui-monospace, Consolas, monospace)" },
      { tag: [tags.escape, tags.regexp], color: "var(--muted)" },
      { tag: [tags.atom, tags.bool, tags.null], color: "var(--muted)" },
      { tag: [tags.operator, tags.operatorKeyword], color: "var(--muted-foreground)" },
      { tag: [tags.punctuation, tags.bracket], color: "var(--muted-foreground)" },
      { tag: [tags.namespace, tags.labelName], color: "var(--secondary)" },
      { tag: tags.meta, color: "var(--muted-foreground)", fontStyle: "italic" },
    ]),
  );
  return [editorTheme, highlight];
}

  $effect(() => {
    // `host` is read synchronously so the template ref is a tracked
    // dependency: the effect re-runs once the div exists (and after
    // remounts from reload()).
    const hostEl = host;
    const token = ++loadToken;
    status = "loading";
    void (async () => {
      try {
        const [loaded, bytes] = await Promise.all([loadCm(), readFile(repoId, path)]);
        if (token !== loadToken) return;
        cm = loaded;
        currentMeta = meta;
        mount(hostEl, bytes);
        status = "ready";
      } catch (err) {
        if (token !== loadToken) return;
        errorText = String(err);
        status = "error";
      }
    })();
    return () => {
      loadToken++;
      destroyView();
    };
  });

  function destroyView(): void {
    if (view) {
      const v = view as {
        destroy(): void;
        scrollDOM: HTMLElement;
      };
      v.scrollDOM.removeEventListener("scroll", onEditorScroll);
      v.destroy();
      view = null;
    }
    if (scrollRaf !== 0) {
      cancelAnimationFrame(scrollRaf);
      scrollRaf = 0;
    }
  }

  /** Editor scroll → preview follow (rAF-coalesced, preview may be absent). */
  function onEditorScroll(): void {
    if (scrollRaf !== 0) return;
    scrollRaf = requestAnimationFrame(() => {
      scrollRaf = 0;
      const v = view as
        | {
            scrollDOM: { scrollTop: number };
            lineBlockAtHeight(h: number): { from: number };
            defaultLineHeight: number;
            state: { doc: { lineAt(pos: number): { number: number } } };
          }
        | null;
      if (!v || !previewRef) return;
      const block = v.lineBlockAtHeight(v.scrollDOM.scrollTop);
      const line = v.state.doc.lineAt(block.from).number;
      previewRef.syncFromEditor(line - 1, v.defaultLineHeight);
    });
  }

  /** Preview click → move the editor cursor to the block's source line. */
  function jumpToLine(line: number): void {
    const v = view as
      | {
          dispatch(e: unknown): void;
          focus(): void;
          state: { doc: { lines: number; line(n: number): { from: number } } };
        }
      | null;
    if (!v) return;
    const n = Math.max(1, Math.min(line + 1, v.state.doc.lines));
    v.dispatch({ selection: { anchor: v.state.doc.line(n).from }, scrollIntoView: true });
    v.focus();
  }

  /** Current editor text (read by the preview at render time). */
  function getDocText(): string {
    const v = view as { state: { doc: { toString(): string } } } | null;
    return v === null ? "" : v.state.doc.toString();
  }

  // Re-measure CM when the preview toggle changes the editor column width
  // (display:none → flex) so wrapping/gutters stay accurate.
  $effect(() => {
    void previewMode;
    if (view) {
      (view as { requestMeasure(): void }).requestMeasure();
    }
  });

  function mount(hostEl: HTMLDivElement | undefined, bytes: number[]): void {
    if (!hostEl || !cm) return;
    destroyView();
    const c = cm;
    wrapCompartment = new c.Compartment();
    languageCompartment = new c.Compartment();
    const doc = decoder.decode(new Uint8Array(bytes));
    const extensions: import("@codemirror/state").Extension[] = [
      c.basicSetup,
      ...buildTheme(c),
      wrapCompartment.of(editorWrapExt()),
      c.EditorView.updateListener.of((update) => {
        if (update.docChanged) {
          dirty = true;
          docVersion++;
        }
      }),
      // In-editor fallback for contexts without the global keybinds.
      c.Prec.highest(
        c.keymap.of([
          {
            key: "Mod-s",
            preventDefault: true,
            run: () => {
              void save();
              return true;
            },
          },
        ]),
      ),
    ];
    const nextView = new c.EditorView({
      parent: hostEl,
      state: c.EditorState.create({ doc, extensions }),
    });
    view = nextView;
    nextView.scrollDOM.addEventListener("scroll", onEditorScroll);
    // Lazy per-language grammar (async, swapped into its compartment).
    const desc = c.LanguageDescription.matchFilename(c.languages, basename(path));
    if (desc) {
      void desc
        .load()
        .then((support) => {
          if (tokenAlive() && languageCompartment && view) {
            nextView.dispatch({
              effects: languageCompartment.reconfigure(support),
            });
          }
        })
        .catch(() => {
          // Grammar load failures degrade to plain text silently.
        });
    }
  }

  function editorWrapExt(): import("@codemirror/state").Extension {
    return editorPrefs.wrap ? cm!.lineWrapping : [];
  }

  function tokenAlive(): boolean {
    return view !== null;
  }

  /** Follows the wrap preference (the toggle lives in the tab strip). */
  $effect(() => {
    const wanted = editorPrefs.wrap;
    if (wrapCompartment && cm && view) {
      (view as { dispatch(e: unknown): void }).dispatch({
        effects: wrapCompartment.reconfigure(wanted ? cm.lineWrapping : []),
      });
    }
  });

  // --- Save / reload -----------------------------------------------------------

  /** Serialize + write. Returns false when a conflict dialog is pending. */
  export async function save(): Promise<boolean> {
    if (!view || !cm || status !== "ready") return false;
    const text = (view as { state: { doc: { toString(): string } } }).state.doc.toString();
    const bytes = encoder.encode(text);
    try {
      if (!checkpointed) {
        await checkpointCreate(repoId, `edit-file: ${path}`);
        checkpointed = true;
      }
      const saved = await writeFile(repoId, path, bytes, currentMeta.mtime_ms);
      currentMeta = saved;
      dirty = false;
      onSaved?.();
      return true;
    } catch (err) {
      if (String(err).includes("changed on disk")) {
        conflictOpen = true;
      } else {
        toast(String(err), { kind: "error" });
      }
      return false;
    }
  }

  /** Re-read from disk (conflict dialog "Reload"): fresh stat + remount. */
  export async function reload(): Promise<void> {
    const token = ++loadToken;
    try {
      const [bytes, fresh] = await Promise.all([
        readFile(repoId, path),
        import("$lib/ipc/client").then((m) => m.statFile(repoId, path)),
      ]);
      if (token !== loadToken) return;
      currentMeta = fresh;
      checkpointed = false;
      mount(host, bytes);
    } catch (err) {
      toast(String(err), { kind: "error" });
    }
  }

  // --- Conflict dialog -----------------------------------------------------------

  let conflictOpen = $state(false);

  function conflictReload(): void {
    conflictOpen = false;
    dirty = false;
    void reload();
  }

  function conflictKeepMine(): void {
    conflictOpen = false;
    void (async () => {
      try {
        const text = (view as { state: { doc: { toString(): string } } }).state.doc.toString();
        const saved = await writeFile(repoId, path, encoder.encode(text), null);
        currentMeta = saved;
        dirty = false;
        onSaved?.();
      } catch (err) {
        toast(String(err), { kind: "error" });
      }
    })();
  }
</script>

<div class="editor flex h-full min-h-0 min-w-0 flex-col bg-card {showPreview ? 'flex-row' : ''}">
  <!-- The host exists for the whole component lifetime (mounting into a
       node that only appears later would oscillate: ready → host binds →
       effect re-runs → loading → host unbinds). Loading/error state is
       an overlay on top of the (hidden) host instead. -->
  {#if status === "loading"}
    <p class="state m-auto inline-flex items-center gap-[0.4rem] text-[0.8125rem] text-muted-foreground" role="status"><Spinner class="size-[14px]" role="presentation" aria-hidden="true" /> Loading {path}…</p>
  {:else if status === "error"}
    <p class="state m-auto inline-flex items-center gap-[0.4rem] text-[0.8125rem] text-destructive" role="alert">{errorText}</p>
  {/if}
  <div
    class="cm-host flex-1 min-h-0 overflow-hidden {showPreview ? 'min-w-0' : ''} {status !== "ready" || editorHidden ? 'hidden' : ''}"
    bind:this={host}
  ></div>
  {#if showPreview}
    <div class="preview-host flex-[1_1_50%] min-w-0 min-h-0 border-l border-border {editorHidden ? 'border-l-0' : ''}">
      {#await import("./MarkdownPreview.svelte") then { default: Preview }}
        <Preview
          bind:this={previewRef}
          {path}
          getDoc={getDocText}
          version={docVersion}
          onJumpToLine={jumpToLine}
        />
      {/await}
    </div>
  {/if}

  <ConfirmDialog
    bind:open={conflictOpen}
    title="File changed on disk"
    message="{path} was modified outside this window while you were editing. Reload the on-disk version, or keep your edits and overwrite the file?"
    confirmLabel="Reload from disk"
    cancelLabel="Keep my version"
    onConfirm={conflictReload}
    onCancel={conflictKeepMine}
  />
</div>

<style>
  /*
   * Deliberate residual CSS: CodeMirror-generated DOM (`.cm-editor` is
   * created by the CM view, not this template), so the rules can't live in
   * markup utilities. Everything else in this component is Tailwind.
   */
  .cm-host :global(.cm-editor) {
    height: 100%;
  }

  .cm-host :global(.cm-editor.cm-focused) {
    outline: none;
  }
</style>
