import { invoke } from '@tauri-apps/api/core';

export async function startAcceptingConnections(): Promise<string> {
  return invoke('start_accepting_connections');
} 