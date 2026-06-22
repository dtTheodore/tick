import { toastStore } from '@/lib/toast';
import { cn } from '@/lib/utils';
import { useSyncExternalStore } from 'react';

export function Toaster() {
  const list = useSyncExternalStore(toastStore.subscribe, toastStore.getSnapshot);
  return (
    <div className="pointer-events-none fixed bottom-20 left-1/2 z-50 flex -translate-x-1/2 flex-col gap-1">
      {list.map((t) => (
        <div
          key={t.id}
          className={cn(
            'rounded px-3 py-1.5 font-mono text-xs shadow-lg',
            t.kind === 'info'
              ? 'bg-white/10 text-white'
              : t.kind === 'warn'
                ? 'bg-yellow-500/20 text-yellow-300'
                : 'bg-tick-loss/30 text-tick-loss',
          )}
        >
          {t.text}
        </div>
      ))}
    </div>
  );
}
