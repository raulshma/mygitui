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
    the M3 theme is built from `--m3-*` CSS variables so it follows the
    app's dynamic color scheme automatically.
-->
<script lang="ts">
  import type { FileMeta } from "$lib/ipc/types";
  import { checkpointCreate, readFile, writeFile } from "$lib/ipc/client";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import { Spinner } from "$lib/components/m3";
  import { toast } from "$lib/toast";
  import { tags } from "@lezer/highlight";
  import { editorPrefs } from "./editorPrefs.svelte";

  let {
    repoId,
    path,
    meta,
    dirty = $bindable(false),
    onSaved = undefined,
  }: {
    repoId: string;
    /** Workdir-relative path (forward slashes). */
    path: string;
    /** Fresh stat from open time (mtime guard anchor). */
    meta: FileMeta;
    dirty?: boolean;
    /** After a successful save (workspace refreshes status). */
    onSaved?: () => void;
  } = $props();

  let status = $state<"loading" | "ready" | "error">("loading");
  let errorText = $state("");
  let host: HTMLDivElement | undefined = $state();

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

  /** M3-flavored editor + syntax theme (CSS variables → follows theme). */
  function buildTheme(c: Cm) {
    const editorTheme = c.EditorView.theme({
      "&": {
        color: "var(--m3-on-surface)",
        backgroundColor: "var(--m3-surface)",
        fontSize: "0.8125rem",
      },
      ".cm-content": {
        caretColor: "var(--m3-primary)",
        fontFamily: "var(--app-font-mono, ui-monospace, Consolas, monospace)",
      },
      ".cm-scroller": {
        fontFamily: "var(--app-font-mono, ui-monospace, Consolas, monospace)",
        lineHeight: "1.5",
      },
      ".cm-gutters": {
        backgroundColor: "var(--m3-surface-container, var(--m3-surface))",
        color: "var(--m3-outline, var(--m3-on-surface-variant))",
        border: "none",
        borderRight: "1px solid var(--app-border, var(--m3-outline-variant))",
      },
      ".cm-activeLine": {
        backgroundColor:
          "color-mix(in srgb, var(--m3-primary) 7%, transparent)",
      },
      ".cm-activeLineGutter": {
        backgroundColor:
          "color-mix(in srgb, var(--m3-primary) 12%, transparent)",
        color: "var(--m3-on-surface)",
      },
      "&.cm-focused .cm-selectionBackground, .cm-selectionBackground":
        {
          backgroundColor:
            "color-mix(in srgb, var(--m3-primary) 22%, transparent) !important",
        },
      ".cm-selectionMatch": {
        backgroundColor:
          "color-mix(in srgb, var(--m3-tertiary) 25%, transparent)",
      },
      ".cm-searchMatch": {
        backgroundColor:
          "color-mix(in srgb, var(--m3-tertiary) 30%, transparent)",
        outline: "1px solid var(--m3-tertiary)",
      },
      ".cm-searchMatch-selected": {
        backgroundColor:
          "color-mix(in srgb, var(--m3-primary) 35%, transparent)",
      },
      ".cm-panels": {
        backgroundColor: "var(--m3-surface-container, var(--m3-surface))",
        color: "var(--m3-on-surface)",
        borderBottom: "1px solid var(--app-border, var(--m3-outline-variant))",
      },
      ".cm-panel.cm-search input, .cm-panel.cm-search button": {
        background: "var(--m3-surface)",
        color: "var(--m3-on-surface)",
        border: "1px solid var(--app-border, var(--m3-outline-variant))",
        borderRadius: "4px",
        font: "inherit",
        padding: "0.05rem 0.3rem",
      },
      ".cm-tooltip": {
        backgroundColor: "var(--m3-surface-container-high)",
        color: "var(--m3-on-surface)",
        border: "1px solid var(--app-border, var(--m3-outline-variant))",
      },
    });
    const highlight = c.syntaxHighlighting(
      c.HighlightStyle.define([
        { tag: tags.keyword, color: "var(--m3-primary)" },
        { tag: [tags.controlKeyword, tags.moduleKeyword], color: "var(--m3-primary)" },
        { tag: tags.string, color: "var(--m3-tertiary)" },
        { tag: tags.number, color: "var(--m3-tertiary)" },
        {
          tag: tags.comment,
          color: "var(--m3-outline, var(--m3-on-surface-variant))",
          fontStyle: "italic",
        },
        { tag: [tags.function(tags.variableName), tags.function(tags.propertyName)], color: "var(--m3-secondary)" },
        { tag: [tags.typeName, tags.className], color: "var(--m3-secondary)" },
        { tag: [tags.variableName, tags.propertyName], color: "var(--m3-on-surface)" },
        { tag: tags.heading, color: "var(--m3-primary)", fontWeight: "bold" },
        { tag: tags.link, color: "var(--m3-tertiary)", textDecoration: "underline" },
        { tag: tags.invalid, color: "var(--m3-error)" },
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
      (view as { destroy(): void }).destroy();
      view = null;
    }
  }

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
        if (update.docChanged) dirty = true;
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

<div class="editor">
  <!-- The host exists for the whole component lifetime (mounting into a
       node that only appears later would oscillate: ready → host binds →
       effect re-runs → loading → host unbinds). Loading/error state is
       an overlay on top of the (hidden) host instead. -->
  {#if status === "loading"}
    <p class="state" role="status"><Spinner size={14} /> Loading {path}…</p>
  {:else if status === "error"}
    <p class="state state-error" role="alert">{errorText}</p>
  {/if}
  <div class="cm-host" class:hidden={status !== "ready"} bind:this={host}></div>

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
  .editor {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    min-width: 0;
    background: var(--m3-surface);
  }

  .cm-host {
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  .cm-host.hidden {
    display: none;
  }

  .cm-host :global(.cm-editor) {
    height: 100%;
  }

  .cm-host :global(.cm-editor.cm-focused) {
    outline: none;
  }

  .state {
    margin: auto;
    color: var(--m3-on-surface-variant);
    font-size: 0.8125rem;
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
  }

  .state-error {
    color: var(--m3-error);
  }
</style>
