<!--
  The checked-out loans list: rows link to the book's catalog page, covers
  fall back to a placeholder, Libby items get a chip instead of Renew.
-->
<script lang="ts">
  import type { Loan } from "$lib/types";
  import { convertFileSrc } from "@tauri-apps/api/core";

  // Covers are local file paths (downloaded by the cache) or remote URLs;
  // local files load through the asset: protocol, URLs pass through.
  function coverSrc(cover?: string) {
    if (!cover) return "";
    return cover.startsWith("/") ? convertFileSrc(cover) : cover;
  }

  export let loans: Loan[];
  export let dimmed: boolean; // list dimmed while a refresh/restore is in flight
  export let renewDisabled: boolean;
  export let renewing: Record<string, boolean>; // spinner state keyed by recordId
  export let onrenew: (item: Loan) => void;
  export let onopen: (item: Loan) => void;
  export let onlibby: () => void;

  let brokenCovers: Record<string, boolean> = {}; // 📖 fallback after load errors

  function markBroken(url: string) {
    if (!url) return;
    brokenCovers = { ...brokenCovers, [url]: true };
  }
</script>

<ul class="loans" class:busy={dimmed}>
  {#each loans as item, i ('loan-' + i)}
    <li class:overdue={item.overdue}>
      <a
        class="pill-body"
        href={item.url || ""}
        onclick={(e) => {
          e.preventDefault();
          onopen(item);
        }}
        title="View in the library catalog"
      >
        {#if item.cover && !brokenCovers[item.cover]}
          <img
            class="cover"
            src={coverSrc(item.cover)}
            alt=""
            loading="lazy"
            onerror={() => markBroken(item.cover ?? "")}
          />
        {:else}
          <div class="cover placeholder" aria-hidden="true">📖</div>
        {/if}
        <div class="meta">
          <span class="title" title={item.title}>{item.title || "Untitled item"}</span>
          <span class="due" class:overdue={item.overdue}>
            {item.overdue ? "Overdue" : ""} {item.due}
          </span>
        </div>
      </a>
      {#if item.libby}
        <button class="libby-tag" onclick={onlibby} title="Checked out via Libby">Libby</button>
      {:else if item.renewable}
        <button
          class="btn renew"
          onclick={() => onrenew(item)}
          disabled={renewDisabled || !!renewing[item.recordId]}
        >
          {renewing[item.recordId] ? "…" : "Renew"}
        </button>
      {/if}
    </li>
  {/each}
</ul>

<style>
  .loans {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
    overflow-y: auto;
    flex: 1;
    /* min-height: 0 lets the flex item shrink to the panel height and
       scroll; without it the list overflows the fixed-size panel. */
    min-height: 0;
  }
  .loans.busy {
    opacity: 0.6;
  }
  .loans li {
    display: flex;
    align-items: center;
    gap: 8px;
    background: var(--row-bg);
    border: 1px solid var(--row-border);
    border-radius: 10px;
    padding: 8px 10px;
  }
  .loans li:hover {
    background: var(--row-bg-hover);
  }
  .loans li.overdue {
    border-color: color-mix(in srgb, var(--danger) 45%, transparent);
  }
  .pill-body {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    color: inherit;
    text-decoration: none;
    border-radius: 6px;
  }
  .cover {
    width: 40px;
    height: 56px;
    object-fit: cover;
    border-radius: 6px;
    flex-shrink: 0;
    background: var(--row-bg);
    margin-right: 6px;
  }
  .cover.placeholder {
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 18px;
  }
  .meta {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .title {
    font-size: 13px;
    font-weight: 550;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .due {
    font-size: 11.5px;
    color: var(--text-2);
  }
  .due.overdue {
    color: var(--danger);
    font-weight: 600;
  }
  .btn.renew {
    flex-shrink: 0;
    padding: 5px 10px;
  }
  .libby-tag {
    flex-shrink: 0;
    font-size: 11px;
    font-weight: 650;
    letter-spacing: 0.02em;
    color: var(--libby-text);
    background: var(--libby-bg);
    border: 1px solid var(--libby-border);
    border-radius: 999px;
    padding: 4px 10px;
    cursor: pointer;
  }
  .libby-tag:hover {
    background: var(--libby-bg-hover);
  }
</style>