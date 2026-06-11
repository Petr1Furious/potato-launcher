/// <reference types="vite/client" />

export {};

declare global {
  interface ImportMetaEnv {
    readonly VITE_LAUNCHER_NAME?: string;
    readonly VITE_GITHUB_URL?: string;
  }

  interface ImportMeta {
    readonly env: ImportMetaEnv;
  }

  interface Window {
    config?: {
      VITE_GITHUB_URL?: string;
      VITE_LAUNCHER_NAME?: string;
    };
  }
}
