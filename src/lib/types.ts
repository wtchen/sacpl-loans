// Shared types used by the panel page and its components.

export type LibStatus = { ready: boolean; loggedIn: boolean; loginError?: string };

export type Loan = {
  title: string;
  due: string;
  cover?: string; // catalog bookcover URL, or a local path to the downloaded cover file
  url?: string; // the book's catalog page (opened on click)
  libby?: boolean; // OverDrive/Libby item — renewals happen in the Libby app
  overdue: boolean;
  kind: string; // "ils" (physical) | "overdrive" (Libby ebook) | "" (not renewable)
  patronId: string;
  recordId: string;
  renewIndicator: string;
  renewable: boolean;
};

export type Checkouts = {
  success: boolean;
  message: string;
  lastLoaded?: string;
  count?: number;
  items?: Loan[];
  rawHtmlLength?: number;
  stillSyncing?: boolean;
  cached?: boolean; // true when served from the on-disk cache
  savedAt?: number; // cache write time, unix seconds
};

export type RenewResult = { success: boolean; title: string; message: string; renewed: number };

export type AppSettings = {
  refreshSecs: number;
  bridgeVisible: boolean;
  debugMode: boolean; // true in debug builds (cargo feature "debug-mode")
};

export type ToastKind = "ok" | "err";