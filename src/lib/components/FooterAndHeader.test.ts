// Smoke tests for the refresh footer and the account header.
import { render, screen, fireEvent } from "@testing-library/svelte";
import { describe, expect, it, vi } from "vitest";

import RefreshFooter from "./RefreshFooter.svelte";
import AccountHeader from "./AccountHeader.svelte";

describe("RefreshFooter", () => {
  it("shows the last-updated label with its tooltip and triggers refreshes", () => {
    const onrefresh = vi.fn();
    render(RefreshFooter, {
      busy: false,
      reconnecting: false,
      updated: "Updated 10:15 PM",
      updatedTitle: "Sep 6 10:15 PM",
      onrefresh,
    });
    const label = screen.getByText("Updated 10:15 PM");
    expect(label.getAttribute("title")).toBe("Sep 6 10:15 PM");
    expect(screen.getByRole("button", { name: "Refresh" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(onrefresh).toHaveBeenCalledTimes(1);
  });

  it("disables the button while a refresh or restore is in flight", () => {
    const { rerender } = render(RefreshFooter, {
      busy: true,
      reconnecting: false,
      updated: "",
      updatedTitle: "",
      onrefresh: () => {},
    });
    expect((screen.getByRole("button", { name: "Refreshing…" }) as HTMLButtonElement).disabled).toBe(true);
    rerender({ busy: false, reconnecting: true });
    expect((screen.getByRole("button", { name: "Refresh" }) as HTMLButtonElement).disabled).toBe(true);
  });
});

describe("AccountHeader", () => {
  it("summarizes the loans and wires the header actions", () => {
    const onsettings = vi.fn();
    const onlogout = vi.fn();
    const onquit = vi.fn();
    render(AccountHeader, {
      loanCount: 3,
      overdueCount: 1,
      busy: false,
      onsettings,
      onlogout,
      onquit,
    });
    expect(screen.getByText("3 checked out · 1 overdue")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Sign Out" }));
    fireEvent.click(screen.getByRole("button", { name: "Quit" }));
    expect(onsettings).toHaveBeenCalledTimes(1);
    expect(onlogout).toHaveBeenCalledTimes(1);
    expect(onquit).toHaveBeenCalledTimes(1);
  });

  it("disables Sign Out while busy", () => {
    render(AccountHeader, {
      loanCount: 0,
      overdueCount: 0,
      busy: true,
      onsettings: () => {},
      onlogout: () => {},
      onquit: () => {},
    });
    expect((screen.getByRole("button", { name: "Sign Out" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText("0 checked out")).toBeTruthy();
  });
});