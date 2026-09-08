// Smoke tests for list placeholder states and the sign-in form.
import { render, screen, fireEvent } from "@testing-library/svelte";
import { describe, expect, it, vi } from "vitest";

import ListState from "./ListState.svelte";
import LoginView from "./LoginView.svelte";

describe("ListState", () => {
  it("shows the loading and empty states", () => {
    const { unmount } = render(ListState, { kind: "loading", onretry: () => {} });
    expect(screen.getByText("Loading your loans from the library…")).toBeTruthy();
    unmount();
    render(ListState, { kind: "empty", onretry: () => {} });
    expect(screen.getByText("No items checked out.")).toBeTruthy();
  });

  it("offers a retry while the ILS is still syncing", () => {
    const onretry = vi.fn();
    render(ListState, { kind: "syncing", onretry });
    expect(screen.getByText("The library is still syncing your loans.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(onretry).toHaveBeenCalledTimes(1);
  });
});

describe("LoginView", () => {
  function props(overrides: Record<string, unknown> = {}) {
    return {
      busy: false,
      loginError: "",
      onlogin: vi.fn(),
      onquit: vi.fn(),
      ...overrides,
    };
  }

  it("rejects empty submissions without signing in", () => {
    const onlogin = vi.fn();
    const { container } = render(LoginView, props({ onlogin }));
    fireEvent.submit(container.querySelector("form")!);
    expect(screen.getByRole("alert").textContent).toBe("Enter your library ID and PIN.");
    expect(onlogin).not.toHaveBeenCalled();
  });

  it("signs in with trimmed credentials", () => {
    const onlogin = vi.fn();
    const { container } = render(LoginView, props({ onlogin }));
    const user = screen.getByLabelText(/Library ID/i) as HTMLInputElement;
    const pin = screen.getByLabelText(/PIN/i) as HTMLInputElement;
    fireEvent.input(user, { target: { value: " 1234567 " } });
    fireEvent.input(pin, { target: { value: "1234" } });
    fireEvent.submit(container.querySelector("form")!);
    expect(onlogin).toHaveBeenCalledWith("1234567", "1234");
  });

  it("disables the form while busy and surfaces sign-in errors", () => {
    render(LoginView, props({ busy: true, loginError: "Invalid PIN" }));
    expect((screen.getByLabelText(/Library ID/i) as HTMLInputElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Signing in…" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText("Invalid PIN")).toBeTruthy();
  });
});