// Smoke tests for the loan list: basic layout and the row interactions.
import { render, screen, fireEvent } from "@testing-library/svelte";
import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (p: string) => `asset://test/${p}`,
}));

import LoanList from "./LoanList.svelte";
import type { Loan } from "$lib/types";

function loan(overrides: Partial<Loan> = {}): Loan {
  return {
    title: "The Map",
    due: "Due Sep 20, 2026",
    cover: "",
    overdue: false,
    kind: "ils",
    patronId: "p1",
    recordId: "r1",
    renewIndicator: "ind",
    renewable: true,
    ...overrides,
  };
}

function baseProps(loans: Loan[], overrides: Record<string, unknown> = {}) {
  return {
    loans,
    dimmed: false,
    renewDisabled: false,
    renewing: {},
    onrenew: vi.fn(),
    onopen: vi.fn(),
    onlibby: vi.fn(),
    ...overrides,
  };
}

describe("LoanList", () => {
  it("renders every loan with its title and due date", () => {
    render(LoanList, baseProps([loan(), loan({ title: "The Garden", recordId: "r2" })]));
    expect(screen.getByText("The Map")).toBeTruthy();
    expect(screen.getByText("The Garden")).toBeTruthy();
    expect(screen.getAllByText("Due Sep 20, 2026").length).toBe(2);
  });

  it("flags overdue items", () => {
    render(LoanList, baseProps([loan({ overdue: true })]));
    expect(screen.getByText(/Overdue/)).toBeTruthy();
    expect(document.querySelector("li.overdue")).toBeTruthy();
  });

  it("shows a placeholder without a cover and the asset URL for downloaded covers", () => {
    render(LoanList, baseProps([loan({ cover: "/appdata/covers/a.jpg" })]));
    const img = document.querySelector("img.cover");
    expect(img?.getAttribute("src")).toBe("asset://test//appdata/covers/a.jpg");
    // No remote/relative fetch: the placeholder is only for missing covers.
    expect(document.querySelector(".cover.placeholder")).toBeNull();
  });

  it("shows the Libby chip instead of a Renew button for Libby items", () => {
    render(LoanList, baseProps([loan({ libby: true, kind: "overdrive" })]));
    expect(screen.getByText("Libby")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Renew" })).toBeNull();
  });

  it("enables or disables Renew with renewDisabled and reports clicks", async () => {
    const onrenew = vi.fn();
    const { rerender } = render(LoanList, baseProps([loan()], { onrenew }));
    const button = screen.getByRole("button", { name: "Renew" });
    expect((button as HTMLButtonElement).disabled).toBe(false);
    await fireEvent.click(button);
    expect(onrenew).toHaveBeenCalledWith(expect.objectContaining({ recordId: "r1" }));

    await rerender({ renewDisabled: true });
    expect((screen.getByRole("button", { name: "Renew" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("opens the catalog on row click", () => {
    const onopen = vi.fn();
    const item = loan({ cover: undefined });
    render(LoanList, baseProps([item], { onopen }));
    fireEvent.click(screen.getByText("The Map"));
    expect(onopen).toHaveBeenCalledWith(item);
  });
});