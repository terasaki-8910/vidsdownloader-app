import { useCallback, useEffect, useState } from "react";

export interface ListSelectionState<Id> {
  selected: Set<Id>;
  anchor: Id | null;
  focus: Id | null;
}

export type ListSelectionAction<Id> =
  | { type: "sync" }
  | { type: "click"; id: Id; mod: boolean; shift: boolean }
  | { type: "context"; id: Id }
  | { type: "move"; id: Id; mod: boolean; shift: boolean }
  | { type: "space"; id: Id; mod: boolean }
  | { type: "selectAll" }
  | { type: "clear" }
  | { type: "set"; ids: Id[]; focus?: Id | null };

function range<Id>(ids: readonly Id[], from: Id, to: Id): Id[] {
  const a = ids.indexOf(from);
  const b = ids.indexOf(to);
  if (a < 0 || b < 0) return [to];
  return ids.slice(Math.min(a, b), Math.max(a, b) + 1);
}

/** Pure Explorer-style selection transition shared by both ordered lists. */
export function transitionListSelection<Id>(
  ids: readonly Id[],
  state: ListSelectionState<Id>,
  action: ListSelectionAction<Id>,
): ListSelectionState<Id> {
  const present = new Set(ids);
  const kept = new Set([...state.selected].filter((id) => present.has(id)));
  const anchor = state.anchor != null && present.has(state.anchor) ? state.anchor : null;
  const focus = state.focus != null && present.has(state.focus) ? state.focus : null;

  if (action.type === "sync") return { selected: kept, anchor, focus };
  if (action.type === "clear") return { selected: new Set(), anchor: null, focus };
  if (action.type === "selectAll") {
    return { selected: new Set(ids), anchor: anchor ?? focus ?? ids[0] ?? null, focus: focus ?? ids[0] ?? null };
  }
  if (action.type === "set") {
    const selected = new Set(action.ids.filter((id) => present.has(id)));
    const nextFocus = action.focus !== undefined ? action.focus : action.ids[action.ids.length - 1] ?? null;
    const validFocus = nextFocus != null && present.has(nextFocus) ? nextFocus : null;
    return { selected, anchor: action.ids.find((id) => present.has(id)) ?? validFocus, focus: validFocus };
  }
  if (!present.has(action.id)) return { selected: kept, anchor, focus };

  if (action.type === "context") {
    return kept.has(action.id)
      ? { selected: kept, anchor, focus: action.id }
      : { selected: new Set([action.id]), anchor: action.id, focus: action.id };
  }

  if (action.type === "move") {
    if (action.shift) {
      const base = anchor ?? focus ?? action.id;
      const selected = action.mod ? new Set(kept) : new Set<Id>();
      for (const id of range(ids, base, action.id)) selected.add(id);
      return { selected, anchor: base, focus: action.id };
    }
    if (action.mod) return { selected: kept, anchor, focus: action.id };
    return { selected: new Set([action.id]), anchor: action.id, focus: action.id };
  }

  if (action.type === "space") {
    if (!action.mod) return { selected: new Set([action.id]), anchor: action.id, focus: action.id };
    const selected = new Set(kept);
    if (selected.has(action.id)) selected.delete(action.id);
    else selected.add(action.id);
    return { selected, anchor: action.id, focus: action.id };
  }

  if (action.shift) {
    const base = anchor ?? focus ?? action.id;
    const selected = action.mod ? new Set(kept) : new Set<Id>();
    for (const id of range(ids, base, action.id)) selected.add(id);
    return { selected, anchor: base, focus: action.id };
  }
  if (action.mod) {
    const selected = new Set(kept);
    if (selected.has(action.id)) selected.delete(action.id);
    else selected.add(action.id);
    return { selected, anchor: action.id, focus: action.id };
  }
  return { selected: new Set([action.id]), anchor: action.id, focus: action.id };
}

export function useListSelection<Id>(ids: readonly Id[]) {
  const [state, setState] = useState<ListSelectionState<Id>>({ selected: new Set(), anchor: null, focus: null });

  useEffect(() => {
    setState((current) => transitionListSelection(ids, current, { type: "sync" }));
  }, [ids]);

  const act = useCallback(
    (action: ListSelectionAction<Id>) => setState((current) => transitionListSelection(ids, current, action)),
    [ids],
  );

  return { ...state, act };
}
