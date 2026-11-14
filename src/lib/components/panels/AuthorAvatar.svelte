<script lang="ts">
  /**
   * AuthorAvatar — the VSCode Source Control Graph's initials avatar:
   * a deterministic-hue circle (no network fetch) with the author's
   * initials. Wrap it in a `Hint` at the call site to expose the full
   * identity (`Name <email>`) on hover.
   */
  import { authorAvatarColor, authorInitials } from "$lib/git/format";

  let {
    name,
    email = "",
    /** Rendered diameter in px (28px history rows use 16). */
    size = 16,
  }: {
    name: string;
    email?: string;
    size?: number;
  } = $props();

  const initials = $derived(authorInitials(name));
  const color = $derived(authorAvatarColor(email, name));
  const fontSize = $derived(Math.max(8, Math.round(size * 0.55)));
</script>

<span
  class="avatar inline-flex flex-none select-none items-center justify-center rounded-full font-semibold leading-none text-white"
  style:width="{size}px"
  style:height="{size}px"
  style:background={color}
  style:font-size="{fontSize}px"
  aria-hidden="true"
>{initials}</span>
