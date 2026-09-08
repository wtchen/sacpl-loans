// Smoke tests for the Debug Events panel and the connecting state.
import { render, screen, fireEvent } from "@testing-library/svelte";
import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(""),
  convertFileSrc: (p: string) => `asset://test/${p}`,
}));

import DebugEvents from "./DebugEvents.svelte";
import ConnectingView from "./ConnectingView.svelte";
import ReconnectingBanner from "./ReconnectingBanner.svelte";
import Toast from "./Toast.svelte";

describe("DebugEvents", () => {
  it("offers to enter debug mode when inactive", () => {
    const onactivate = vi.fn();
    render(DebugEvents, { debugActive: false, onactivate, onadd: () => {} });
    expect(screen.queryByText("Fake Books")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Enter Debug Mode" }));
    expect(onactivate).toHaveBeenCalledTimes(1);
  });

  it("lists fake books, refresh-time and catalog scenarios when active", () => {
    const onadd = vi.fn();
    render(DebugEvents, { debugActive: true, onactivate: () => {}, onadd });
    expect(screen.getByText("Fake Books")).toBeTruthy();
    expect(screen.getByText("Refresh Time")).toBeTruthy();
    expect(screen.getByText("Catalog Events")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Refreshed just now" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Catalog returns 403" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Add Libby book" }));
    const added = onadd.mock.calls[0][0] as Record<string, unknown>;
    expect(added.libby).toBe(true);
    expect(added.kind).toBe("overdrive");
    expect(String(added.recordId)).toMatch(/^debug-/);
    expect(String(added.due)).toMatch(/^Due /);
  });
});

describe("ConnectingView and session status views", () => {
  it("shows the connecting state with a quit action", () => {
    const onquit = vi.fn();
    render(ConnectingView, { onquit });
    expect(screen.getByText("Connecting to the catalog…")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Quit" }));
    expect(onquit).toHaveBeenCalledTimes(1);
  });

  it("shows the re-authenticating banner", () => {
    render(ReconnectingBanner);
    expect(screen.getByText(/signing you back in/)).toBeTruthy();
  });
});

describe("Toast", () => {
  it("renders a message with its kind and nothing when empty", () => {
    const { container, unmount } = render(Toast, { msg: "Renewed!", kind: "ok" });
    expect(screen.getByText("Renewed!")).toBeTruthy();
    expect(container.querySelector(".toast.ok")).toBeTruthy();
    unmount();
    render(Toast, { msg: "", kind: "ok" });
    expect(container.querySelector(".toast")).toBeNull();
  });
});