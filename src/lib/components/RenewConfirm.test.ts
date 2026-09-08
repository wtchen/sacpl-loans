// Smoke tests for the renewal confirmation dialog.
import { render, screen, fireEvent } from "@testing-library/svelte";
import { describe, expect, it, vi } from "vitest";

import RenewConfirm from "./RenewConfirm.svelte";
import type { Loan } from "$lib/types";

const item: Loan = {
  title: "The Map",
  due: "Due Sep 20, 2026",
  overdue: false,
  kind: "ils",
  patronId: "p1",
  recordId: "r1",
  renewIndicator: "ind",
  renewable: true,
};

describe("RenewConfirm", () => {
  it("names the item and offers Cancel/Renew", () => {
    render(RenewConfirm, { item, busy: false, oncancel: () => {}, onconfirm: () => {} });
    expect(screen.getByRole("dialog")).toBeTruthy();
    expect(screen.getByText("The Map")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Renew" })).toBeTruthy();
  });

  it("dismisses via Cancel and the backdrop", () => {
    const oncancel = vi.fn();
    const { container } = render(RenewConfirm, { item, busy: false, oncancel, onconfirm: () => {} });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(oncancel).toHaveBeenCalledTimes(1);
    fireEvent.mouseDown(container.querySelector(".backdrop")!);
    expect(oncancel).toHaveBeenCalledTimes(2);
  });

  it("confirms, and shows a spinner while the renewal runs", async () => {
    const onconfirm = vi.fn();
    const { rerender } = render(RenewConfirm, { item, busy: false, oncancel: () => {}, onconfirm });
    await fireEvent.click(screen.getByRole("button", { name: "Renew" }));
    expect(onconfirm).toHaveBeenCalledTimes(1);

    await rerender({ busy: true });
    const renew = screen.getByRole("button", { name: "Renewing…" }) as HTMLButtonElement;
    expect(renew.disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Cancel" }) as HTMLButtonElement).disabled).toBe(true);
  });
});