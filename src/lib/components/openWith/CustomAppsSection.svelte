<!--
  CustomAppsSection — manager UI for user-defined "Open with" actions.

  Self-contained on purpose (no dialog chrome): CustomAppsDialog wraps it
  for the header menu's "Manage custom apps…" item, and a future Settings
  section can embed it directly. Everything goes through the `customApps`
  store (localStorage `mygitui.openwith.custom`); the command template's
  `{path}` is substituted with the repository root at launch — a template
  without `{path}` gets the root appended as the final argument (backend:
  editors.rs `open_with_custom`).
-->
<script lang="ts">
  import { customApps, CustomAppsStore, type CustomApp } from "$lib/openwith/customApps.svelte";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import EmptyState from "$lib/components/layout/EmptyState.svelte";
  import { toast } from "$lib/toast";
  import { describeError } from "$lib/errors";

  /**
   * The store to manage. Defaults to the app-wide singleton; injectable for
   * tests (memory storage) so a test never leaks state into the singleton.
   */
  let { store = customApps }: { store?: CustomAppsStore } = $props();

  const iconEdit = $derived(appIcon("edit")!);
  const iconDelete = $derived(appIcon("delete")!);

  // Add/edit form. `editing` null = adding a new app.
  let formOpen = $state(false);
  let editing = $state<CustomApp | null>(null);
  let name = $state("");
  let command = $state("");
  let nameError = $state<string | null>(null);
  let commandError = $state<string | null>(null);

  /** App awaiting the delete confirm (drives the ConfirmDialog). */
  let deleting = $state<CustomApp | null>(null);

  function startAdd(): void {
    editing = null;
    name = "";
    command = "";
    nameError = null;
    commandError = null;
    formOpen = true;
  }

  function startEdit(app: CustomApp): void {
    editing = app;
    name = app.name;
    command = app.command;
    nameError = null;
    commandError = null;
    formOpen = true;
  }

  function cancelForm(): void {
    formOpen = false;
    editing = null;
    nameError = null;
    commandError = null;
    name = "";
    command = "";
  }

  /** Validates inline (name + command required), then writes the store. */
  function save(event: SubmitEvent): void {
    event.preventDefault();
    const trimmedName = name.trim();
    const trimmedCommand = command.trim();
    nameError = trimmedName === "" ? "Name is required." : null;
    commandError = trimmedCommand === "" ? "Command is required." : null;
    if (nameError !== null || commandError !== null) return;
    try {
      if (editing !== null) {
        store.update(editing.id, { name: trimmedName, command: trimmedCommand });
        toast(`Updated custom app ${trimmedName}`, { kind: "success" });
      } else {
        store.add({ name: trimmedName, command: trimmedCommand });
        toast(`Added custom app ${trimmedName}`, { kind: "success" });
      }
      cancelForm();
    } catch (err) {
      // Defensive: the store rejects blank fields; the validation above
      // makes this unreachable through the UI.
      toast(describeError(err), { kind: "error" });
    }
  }

  function onDelete(): void {
    const app = deleting;
    if (!app) return;
    deleting = null;
    store.remove(app.id);
    toast(`Removed custom app ${app.name}`, { kind: "success" });
  }
</script>

<section class="custom-apps flex min-w-0 flex-col gap-3" aria-label="Custom open-with apps">
  <p class="m-0 text-ui-meta text-muted-foreground">
    Add your own tools to the "Open with" menu. The command runs through the
    platform shell with the repository root as its working target.
  </p>

  {#if formOpen}
    <form
      class="flex flex-col gap-2 rounded-xl border border-border bg-muted/30 px-4 pb-4 pt-[0.9rem]"
      aria-label={editing !== null ? `Edit custom app ${editing.name}` : "Add custom app"}
      onsubmit={save}
    >
      <label class="flex flex-col gap-ui-125">
        <span class="text-ui-meta text-muted-foreground">Name</span>
        <Input
          class="rounded"
          type="text"
          placeholder="Neovim (kitty)"
          aria-label="Custom app name"
          aria-invalid={nameError !== null}
          bind:value={name}
        />
        {#if nameError !== null}
          <p class="m-0 text-ui-meta text-destructive" role="alert">{nameError}</p>
        {/if}
      </label>
      <label class="flex flex-col gap-ui-125">
        <span class="text-ui-meta text-muted-foreground">Command</span>
        <Input
          class="rounded font-mono"
          type="text"
          placeholder={"code {path}"}
          aria-label="Custom app command"
          aria-invalid={commandError !== null}
          bind:value={command}
          spellcheck="false"
        />
        <span class="text-ui-meta text-muted-foreground">
          <code class="font-mono">{"{path}"}</code> becomes the repository root — e.g.
          <code class="font-mono">{"code {path}"}</code>,
          <code class="font-mono">{"kitty {path}"}</code>,
          <code class="font-mono">{"C:\\Tools\\gvim.exe {path}"}</code>. Without it,
          the root is appended as the last argument.
        </span>
        {#if commandError !== null}
          <p class="m-0 text-ui-meta text-destructive" role="alert">{commandError}</p>
        {/if}
      </label>
      <div class="flex justify-end gap-2 pt-1">
        <Button variant="outline" type="button" onclick={cancelForm}>Cancel</Button>
        <Button type="submit">{editing !== null ? "Save changes" : "Add app"}</Button>
      </div>
    </form>
  {:else}
    <div class="flex justify-end">
      <Button variant="outline" size="sm" type="button" onclick={startAdd}>
        Add custom app
      </Button>
    </div>
  {/if}

  {#if store.apps.length === 0}
    <EmptyState
      title="No custom apps."
      hint="Add a command above and it will appear in the header's “Open with” menu."
    />
  {:else}
    <ul class="m-0 flex list-none flex-col gap-1 p-0" role="list" aria-label="Custom apps">
      {#each store.apps as app (app.id)}
        <li
          class="flex min-w-0 items-center gap-2 rounded-md border border-border px-2 py-1.5 hover:bg-muted/60"
        >
          <span class="flex min-w-0 flex-1 flex-col gap-ui-1">
            <span class="truncate text-sm font-medium text-foreground">{app.name}</span>
            <span class="truncate font-mono text-ui-meta text-muted-foreground"
              >{app.command}</span
            >
          </span>
          <span class="flex flex-none gap-1">
            <Hint label={`Edit ${app.name}`}>
              {#snippet child({ props })}
                <Button
                  {...props}
                  class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
                  variant="ghost"
                  size="icon-xs"
                  type="button"
                  aria-label={`Edit custom app ${app.name}`}
                  onclick={() => startEdit(app)}
                >
                  <Icon icon={iconEdit} size={16} />
                </Button>
              {/snippet}
            </Hint>
            <Hint label={`Delete ${app.name}`}>
              {#snippet child({ props })}
                <Button
                  {...props}
                  class="relative text-destructive after:absolute after:-inset-1 after:content-[''] hover:bg-destructive/10 hover:text-destructive [&_svg:not([class*='size-'])]:size-4"
                  variant="ghost"
                  size="icon-xs"
                  type="button"
                  aria-label={`Delete custom app ${app.name}`}
                  onclick={() => (deleting = app)}
                >
                  <Icon icon={iconDelete} size={16} />
                </Button>
              {/snippet}
            </Hint>
          </span>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<ConfirmDialog
  bind:open={
    () => deleting !== null,
    (v) => {
      if (!v) deleting = null;
    }
  }
  title={deleting !== null ? `Delete custom app ${deleting.name}?` : ""}
  message="The command is removed from the “Open with” menu. Nothing on disk is touched."
  confirmLabel="Delete app"
  danger
  onConfirm={onDelete}
/>
