type Kind = 'tap' | 'locked' | 'settled' | 'reject' | 'wsDrop' | 'drift';

function log(kind: Kind, payload: Record<string, unknown> = {}) {
  console.info(`[tick] ${kind}`, payload);
}

export const telemetry = {
  tap:     (cellKey: string, mult: number) => log('tap', { cellKey, mult }),
  locked:  (positionId: number, mult: number) => log('locked', { positionId, mult }),
  settled: (positionId: number, status: string) => log('settled', { positionId, status }),
  reject:  (cellKey: string, reason: string) => log('reject', { cellKey, reason }),
  wsDrop:  (reason: string) => log('wsDrop', { reason }),
  drift:   (server: number, client: number) => log('drift', { server, client }),
};
