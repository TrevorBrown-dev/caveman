<script lang="ts">
  import type { Config } from '../lib/types';

  interface Props {
    config: Config;
    onsave: (c: Config) => void;
    onclose: () => void;
  }
  let { config, onsave, onclose }: Props = $props();

  // a copy to edit; the dialog is created afresh each time it opens
  // svelte-ignore state_referenced_locally
  let draft = $state<Config>({ ...config });
  let dialog: HTMLDialogElement;

  $effect(() => {
    dialog?.showModal();
  });
</script>

<dialog bind:this={dialog} onclose={onclose}>
  <form method="dialog" onsubmit={(e) => { e.preventDefault(); onsave({ ...draft }); }}>
    <h2>Settings</h2>
    <label>
      <span>Game folder</span>
      <input type="text" bind:value={draft.game} placeholder="E:\Games\ReSkate" spellcheck="false" />
      <small class="faint">The ReSkate folder (Skate.exe, ReSkateLauncher.exe, Mods).</small>
    </label>
    <label>
      <span>Your name</span>
      <input type="text" bind:value={draft.author} placeholder="Caveman" spellcheck="false" />
      <small class="faint">The author in the built mods' manifest (the mod manager shows it); packs take their own owner name.</small>
    </label>
    <label>
      <span>Deck folder</span>
      <input type="text" bind:value={draft.decks} placeholder={`${draft.game}\\Caveman\\Decks`} spellcheck="false" />
    </label>
    <label>
      <span>Wheel folder</span>
      <input type="text" bind:value={draft.wheels} placeholder={`${draft.game}\\Caveman\\Wheels`} spellcheck="false" />
    </label>
    <details>
      <summary class="dim">Advanced</summary>
      <label>
        <span>Mods folder</span>
        <input type="text" bind:value={draft.out} placeholder={`${draft.game}\\Mods`} spellcheck="false" />
        <small class="faint">Leave empty to build into the game. Another folder only for testing.</small>
      </label>
      <label>
        <span>Item cache</span>
        <input type="text" bind:value={draft.cache} placeholder="%LOCALAPPDATA%\Caveman\cache" spellcheck="false" />
      </label>
      <label>
        <span>Trainer (Live panel)</span>
        <input type="text" bind:value={draft.trainer} placeholder="caveman-trainer.exe beside the app" spellcheck="false" />
        <small class="faint">The FX DLL's companion, caveman-trainer.exe. Leave empty for the one beside the app.</small>
      </label>
    </details>
    <div class="actions">
      <button type="button" class="ghost" onclick={() => dialog.close()}>Cancel</button>
      <button type="submit" class="primary">Save</button>
    </div>
  </form>
</dialog>

<style>
  dialog {
    width: min(560px, calc(100vw - 32px));
    padding: 0;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    background: var(--panel);
    color: var(--text);
    box-shadow: var(--shadow);
  }
  dialog::backdrop {
    background: rgba(0, 0, 0, 0.55);
  }
  form {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 20px;
  }
  h2 {
    margin: 0 0 4px;
    font-size: 18px;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  label span {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-dim);
  }
  details {
    border-top: 1px solid var(--line);
    padding-top: 10px;
  }
  details label {
    margin-top: 12px;
  }
  summary {
    cursor: pointer;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 6px;
  }
</style>
