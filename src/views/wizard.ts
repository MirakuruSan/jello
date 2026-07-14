// M6.5: first-run setup wizard (condensed single-screen form).
import { invoke } from "@tauri-apps/api/core";

export class Wizard {
  private overlay: HTMLElement;

  constructor() {
    this.overlay = document.getElementById("wizard-overlay")!;
    document.getElementById("wiz-skip")?.addEventListener("click", () => this.finish(true));
    document.getElementById("wiz-finish")?.addEventListener("click", () => this.finish(false));
    window.addEventListener("jello:run-wizard", () => this.show());
  }

  /** Show the wizard if setup hasn't been completed yet. */
  async maybeShow(): Promise<void> {
    try {
      const s = await invoke<Record<string, unknown>>("settings_get");
      if (!s.wizardComplete) this.show();
    } catch {
      this.show();
    }
  }

  private show(): void {
    this.overlay.classList.add("visible");
  }

  private val(id: string): string {
    return (document.getElementById(id) as HTMLSelectElement | null)?.value || "";
  }
  private checked(id: string): boolean {
    return (document.getElementById(id) as HTMLInputElement | null)?.checked ?? false;
  }

  private async finish(skipped: boolean): Promise<void> {
    if (!skipped) {
      const adblock = this.checked("wiz-adblock");
      const patch: Record<string, unknown> = {
        wizardComplete: true,
        defaultSearch: this.val("wiz-search"),
        defaultChatbot: this.val("wiz-chatbot"),
        theme: this.val("wiz-theme"),
        adblock,
        searchSuggestions: this.checked("wiz-suggest"),
        restoreSession: this.checked("wiz-restore"),
        updateCheck: this.checked("wiz-update"),
      };
      await invoke("settings_set", { patch }).catch(console.error);
      applyTheme(this.val("wiz-theme"));
      if (adblock) {
        invoke("extensions_install_ubol").catch(console.error);
      }
    } else {
      await invoke("settings_set", { patch: { wizardComplete: true } }).catch(console.error);
    }
    this.overlay.classList.remove("visible");
  }
}

export function applyTheme(theme: string): void {
  const root = document.documentElement;
  if (theme === "light") root.setAttribute("data-theme", "light");
  else if (theme === "dark") root.setAttribute("data-theme", "dark");
  else root.removeAttribute("data-theme"); // system
}
