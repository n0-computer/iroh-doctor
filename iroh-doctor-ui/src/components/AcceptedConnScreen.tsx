import React, { useState, useEffect, useCallback } from 'react';
import { ProgressBar } from './ProgressBar';
import { listen } from '@tauri-apps/api/event';
import { getProgressState } from '../bindings';
import { ScreenWrapper } from './ScreenWrapper';

interface Stats {
  send?: string;
  recv?: string;
  echo?: string;
}

interface AcceptedConnScreenProps {
  onBack: () => void;
}

export const AcceptedConnScreen: React.FC<AcceptedConnScreenProps> = ({ onBack }) => {
  const [message, setMessage] = useState<string>('');
  const [position, setPosition] = useState<number>(0);
  const [length, setLength] = useState<number>(0);
  const [stats, setStats] = useState<Stats>({});
  const [isComplete, setIsComplete] = useState(false);

  // Create a polling function using requestAnimationFrame
  const pollProgress = useCallback(async () => {
    if (!isComplete) {
      try {
        const state = await getProgressState();
        setMessage(state.message);
        setPosition(state.position);
        setLength(state.length);
      } catch (error) {
        console.error('Failed to get progress state:', error);
      }
      requestAnimationFrame(pollProgress);
    }
  }, [isComplete]);

  useEffect(() => {
    requestAnimationFrame(pollProgress);

    const statsUnlisten = listen<[string, string]>('test-stats', ({ payload }) => {
      const [type, value] = payload;
      setStats(prev => ({ ...prev, [type]: value }));
    });

    const completeUnlisten = listen('test-complete', () => {
      setIsComplete(true);
      setLength(0);
      setMessage('');
    });

    return () => {
      statsUnlisten.then(unlisten => unlisten());
      completeUnlisten.then(unlisten => unlisten());
    };
  }, [pollProgress]);

  // Get the stats that have values, in the order they were received
  const completedStats = Object.entries(stats).filter(([_, value]) => value);

  return (
    <ScreenWrapper onBack={onBack} style={{ backgroundColor: 'white' }}>
      <h2 className="text-2xl font-koulen mb-8">Connection Accepted</h2>
      
      {/* Stats display - show all completed stats in order */}
      <div className="space-y-4">
        {completedStats.map(([type, value]) => (
          <div key={type} className="flex justify-between items-center">
            <span className="text-sm uppercase text-irohGray-500">{type}</span>
            <span className="font-spaceMono">{value}</span>
          </div>
        ))}
      </div>
      
      {/* Progress bar shown at bottom while test is running */}
      {!isComplete && length > 0 && (
        <div className="mt-4">
          <ProgressBar
            message={message}
            position={position}
            length={length}
          />
        </div>
      )}
    </ScreenWrapper>
  );
}; 