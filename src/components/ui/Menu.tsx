import React, {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils/cn";

export interface MenuOption {
  value: string;
  label: string;
  disabled?: boolean;
}

interface MenuProps {
  options: MenuOption[];
  className?: string;
  selectedValue: string | null;
  onSelect: (value: string) => void;
  placeholder?: string;
  disabled?: boolean;
  onRefresh?: () => void;
}

interface MenuCoords {
  top: number;
  left: number;
  right: number;
  width: number;
  placement: "below" | "above";
  available: number;
}

const GAP = 4;
const VIEWPORT_PADDING = 8;
const MAX_MENU_HEIGHT = 256; // matches max-h-64

/**
 * A custom listbox that replaces the native-feeling Dropdown. The trigger is a
 * recessed well (no border) with the shared focus ring; the panel is a portaled
 * raised surface (so a card's overflow never clips it) separated by tone and one
 * shadow. Keeps the exact Dropdown prop contract so every call site is unchanged.
 * Keyboard: open on click / Enter / Space / Arrow, then Arrow / Home / End to move,
 * Enter or Space to choose, Escape or Tab to close (focus returns to the trigger).
 */
export const Menu: React.FC<MenuProps> = ({
  options,
  selectedValue,
  onSelect,
  className,
  placeholder,
  disabled = false,
  onRefresh,
}) => {
  const { t } = useTranslation();
  const [isOpen, setIsOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(-1);
  const [coords, setCoords] = useState<MenuCoords | null>(null);
  const [menuWidth, setMenuWidth] = useState<number | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const typeaheadRef = useRef<{ query: string; timer: number | null }>({
    query: "",
    timer: null,
  });
  const listboxId = useId();

  const selectedOption = options.find((o) => o.value === selectedValue);
  const firstEnabled = options.findIndex((o) => !o.disabled);

  const updatePosition = useCallback(() => {
    const el = triggerRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const spaceBelow = window.innerHeight - rect.bottom - GAP;
    const spaceAbove = rect.top - GAP;
    const placement: "below" | "above" =
      spaceBelow < Math.min(MAX_MENU_HEIGHT, 160) && rect.top > spaceBelow
        ? "above"
        : "below";
    setCoords({
      top: placement === "below" ? rect.bottom + GAP : rect.top - GAP,
      left: rect.left,
      right: rect.right,
      width: rect.width,
      placement,
      available: placement === "below" ? spaceBelow : spaceAbove,
    });
  }, []);

  const close = useCallback((returnFocus = true) => {
    setIsOpen(false);
    setActiveIndex(-1);
    if (returnFocus) triggerRef.current?.focus();
  }, []);

  const open = useCallback(() => {
    if (disabled) return;
    onRefresh?.();
    updatePosition();
    const selIdx = options.findIndex((o) => o.value === selectedValue);
    setActiveIndex(
      selIdx >= 0 && !options[selIdx]?.disabled ? selIdx : firstEnabled,
    );
    setIsOpen(true);
  }, [
    disabled,
    onRefresh,
    updatePosition,
    options,
    selectedValue,
    firstEnabled,
  ]);

  // Keep the panel attached to the trigger while open.
  useLayoutEffect(() => {
    if (!isOpen) return;
    updatePosition();
    const onScrollResize = () => updatePosition();
    window.addEventListener("scroll", onScrollResize, true);
    window.addEventListener("resize", onScrollResize);
    return () => {
      window.removeEventListener("scroll", onScrollResize, true);
      window.removeEventListener("resize", onScrollResize);
    };
  }, [isOpen, updatePosition]);

  // Measure the rendered panel so we can keep it on-screen even when it grows
  // wider than the trigger (content width). useLayoutEffect => pre-paint, no
  // flicker between the fallback anchor and the measured one.
  useLayoutEffect(() => {
    if (!isOpen) {
      setMenuWidth(null);
      return;
    }
    if (listRef.current) setMenuWidth(listRef.current.offsetWidth);
  }, [isOpen, coords, options]);

  // Move focus into the list when it opens so arrow keys work immediately.
  useEffect(() => {
    if (isOpen) listRef.current?.focus();
  }, [isOpen]);

  // Close on an outside pointer press.
  useEffect(() => {
    if (!isOpen) return;
    const onPointerDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (
        rootRef.current?.contains(target) ||
        listRef.current?.contains(target)
      ) {
        return;
      }
      close(false);
    };
    document.addEventListener("mousedown", onPointerDown);
    return () => document.removeEventListener("mousedown", onPointerDown);
  }, [isOpen, close]);

  const commit = (index: number) => {
    const option = options[index];
    if (!option || option.disabled) return;
    onSelect(option.value);
    close();
  };

  const moveActive = (delta: number) => {
    if (options.length === 0) return;
    setActiveIndex((current) => {
      let next = current;
      for (let i = 0; i < options.length; i++) {
        next = (next + delta + options.length) % options.length;
        if (!options[next]?.disabled) return next;
      }
      return current;
    });
  };

  // Typeahead: printable keys build a short-lived query and jump to the first
  // enabled option whose label starts with it (a fresh single letter advances
  // past the current option so repeats cycle same-initial entries). The buffer
  // clears after a pause, matching a native listbox.
  const typeahead = (char: string) => {
    const state = typeaheadRef.current;
    if (state.timer) window.clearTimeout(state.timer);
    state.query += char.toLowerCase();
    state.timer = window.setTimeout(() => {
      state.query = "";
      state.timer = null;
    }, 500);

    const query = state.query;
    const start = query.length === 1 ? activeIndex + 1 : activeIndex;
    for (let i = 0; i < options.length; i++) {
      const idx = (start + i + options.length) % options.length;
      const option = options[idx];
      if (!option?.disabled && option.label.toLowerCase().startsWith(query)) {
        setActiveIndex(idx);
        return;
      }
    }
  };

  // Clear any pending typeahead timer on unmount.
  useEffect(
    () => () => {
      if (typeaheadRef.current.timer) {
        window.clearTimeout(typeaheadRef.current.timer);
      }
    },
    [],
  );

  const handleTriggerKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      open();
    }
  };

  const handleListKeyDown = (event: React.KeyboardEvent) => {
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        moveActive(1);
        break;
      case "ArrowUp":
        event.preventDefault();
        moveActive(-1);
        break;
      case "Home":
        event.preventDefault();
        setActiveIndex(firstEnabled);
        break;
      case "End": {
        event.preventDefault();
        for (let i = options.length - 1; i >= 0; i--) {
          if (!options[i]?.disabled) {
            setActiveIndex(i);
            break;
          }
        }
        break;
      }
      case "Enter":
      case " ":
        event.preventDefault();
        commit(activeIndex);
        break;
      case "Escape":
        event.preventDefault();
        close();
        break;
      case "Tab":
        close(false);
        break;
      default:
        if (
          event.key.length === 1 &&
          !event.ctrlKey &&
          !event.metaKey &&
          !event.altKey
        ) {
          typeahead(event.key);
        }
    }
  };

  // Keep the active option in view.
  useEffect(() => {
    if (!isOpen || activeIndex < 0) return;
    const node = listRef.current?.querySelector<HTMLElement>(
      `[data-index="${activeIndex}"]`,
    );
    node?.scrollIntoView({ block: "nearest" });
  }, [isOpen, activeIndex]);

  // Right-anchor the panel to the trigger's right edge (these menus live in the
  // right-hand control column) and grow it leftward, then clamp within the
  // viewport using the measured panel width so a wide panel never runs off.
  const effWidth = menuWidth ?? coords?.width ?? 0;
  const clampedLeft = coords
    ? Math.min(
        Math.max(coords.right - effWidth, VIEWPORT_PADDING),
        window.innerWidth - effWidth - VIEWPORT_PADDING,
      )
    : 0;

  return (
    <div className={cn("relative", className)} ref={rootRef}>
      <button
        ref={triggerRef}
        type="button"
        disabled={disabled}
        aria-haspopup="listbox"
        aria-expanded={isOpen}
        aria-controls={isOpen ? listboxId : undefined}
        onClick={() => (isOpen ? close() : open())}
        onKeyDown={handleTriggerKeyDown}
        className={cn(
          "h-8 w-full rounded-control px-3 grid grid-cols-[1fr_auto] items-center gap-2 text-start text-sm bg-surface-well text-ink transition-colors focus-ring",
          disabled
            ? "opacity-60 cursor-not-allowed"
            : "cursor-pointer hover:bg-control-secondary-hover",
        )}
      >
        <span className={cn("truncate", !selectedOption && "text-ink-subtle")}>
          {selectedOption?.label || placeholder}
        </span>
        <ChevronDown
          className={cn(
            "size-4 text-ink-muted transition-transform",
            isOpen && "rotate-180",
          )}
          aria-hidden="true"
        />
      </button>
      {isOpen &&
        !disabled &&
        coords &&
        createPortal(
          <ul
            ref={listRef}
            id={listboxId}
            role="listbox"
            tabIndex={-1}
            aria-activedescendant={
              activeIndex >= 0 ? `${listboxId}-${activeIndex}` : undefined
            }
            onKeyDown={handleListKeyDown}
            style={{
              position: "fixed",
              top: coords.placement === "below" ? coords.top : undefined,
              bottom:
                coords.placement === "above"
                  ? window.innerHeight - coords.top
                  : undefined,
              left: clampedLeft,
              minWidth: coords.width,
              width: "max-content",
              maxWidth: window.innerWidth - VIEWPORT_PADDING * 2,
              maxHeight: Math.min(
                MAX_MENU_HEIGHT,
                coords.available - VIEWPORT_PADDING,
              ),
            }}
            className="z-[var(--z-menu)] overflow-auto rounded-overlay bg-surface-raised border border-hairline py-1 outline-none"
          >
            {options.length === 0 ? (
              <li className="px-3 py-2 text-sm text-ink-subtle">
                {t("common.noOptionsFound")}
              </li>
            ) : (
              options.map((option, index) => {
                const selected = option.value === selectedValue;
                const active = index === activeIndex;
                return (
                  <li
                    key={option.value}
                    id={`${listboxId}-${index}`}
                    data-index={index}
                    role="option"
                    aria-selected={selected}
                    aria-disabled={option.disabled || undefined}
                    onMouseEnter={() =>
                      !option.disabled && setActiveIndex(index)
                    }
                    onClick={() => commit(index)}
                    className={cn(
                      "min-h-8 px-3 flex items-center gap-2 text-sm cursor-pointer",
                      selected
                        ? "bg-surface-selected text-on-selected"
                        : active
                          ? "bg-control-ghost-hover text-ink"
                          : "text-ink",
                      option.disabled && "opacity-60 cursor-not-allowed",
                    )}
                  >
                    <span className="flex-1 whitespace-nowrap">
                      {option.label}
                    </span>
                    {selected && (
                      <Check className="size-4 shrink-0" aria-hidden="true" />
                    )}
                  </li>
                );
              })
            )}
          </ul>,
          document.body,
        )}
    </div>
  );
};
