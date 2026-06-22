interface Toast { id: number; kind: 'info' | 'warn' | 'error'; text: string; }
let nextId = 1;
let toasts: Toast[] = [];
const listeners = new Set<() => void>();
function emit() { listeners.forEach((cb) => cb()); }

export const toastStore = {
  subscribe(cb: () => void) { listeners.add(cb); return () => listeners.delete(cb); },
  getSnapshot() { return toasts; },
  push(kind: Toast['kind'], text: string) {
    const id = nextId++;
    toasts = [...toasts, { id, kind, text }];
    emit();
    setTimeout(() => {
      toasts = toasts.filter((t) => t.id !== id);
      emit();
    }, 3000);
  },
};

export const toast = {
  info: (s: string) => toastStore.push('info', s),
  warn: (s: string) => toastStore.push('warn', s),
  error: (s: string) => toastStore.push('error', s),
};
