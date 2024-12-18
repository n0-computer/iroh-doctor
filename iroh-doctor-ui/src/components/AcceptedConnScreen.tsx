import React, { useState, useEffect } from 'react';
import { ProgressBar } from './ProgressBar';
import { listen } from '@tauri-apps/api/event';

interface Stats {
  send: string;
  recv: string;
  echo: string;
}

interface AcceptedConnScreenProps {
  onBack: () => void;
}

export const AcceptedConnScreen: React.FC<AcceptedConnScreenProps> = ({ onBack }) => {
  const [message, setMessage] = useState<string>('');
  const [position, setPosition] = useState<number>(0);
  const [length, setLength] = useState<number>(0);
  const [stats, setStats] = useState<Stats>({ send: '', recv: '', echo: '' });
  const [isComplete, setIsComplete] = useState(false);

  useEffect(() => {
    // Listen for progress bar updates
    const progressUnlisten = listen<[string, any]>('progress-update', ({ payload }) => {
      const [type, value] = payload;
      switch (type) {
        case 'message':
          setMessage(value);
          break;
        case 'position':
          setPosition(value);
          break;
        case 'length':
          setLength(value);
          break;
      }
    });

    // Listen for test statistics
    const statsUnlisten = listen<[string, string]>('test-stats', ({ payload }) => {
      const [type, value] = payload;
      setStats(prev => ({ ...prev, [type]: value }));
    });

    // Listen for test completion
    const completeUnlisten = listen('test-complete', () => {
      setIsComplete(true);
      setLength(0); // Remove progress bar
      setMessage('');
    });

    // Cleanup listeners
    return () => {
      progressUnlisten.then(unlisten => unlisten());
      statsUnlisten.then(unlisten => unlisten());
      completeUnlisten.then(unlisten => unlisten());
    };
  }, []);

  return (
    <div className="w-full">
      <button 
        onClick={onBack}
        className="mb-8 text-irohGray-500 hover:text-irohGray-800 transition flex items-center gap-2"
      >
        ← Back
      </button>

      <h2 className="text-2xl font-koulen mb-8">Connection Accepted</h2>
      
      {/* Stats display */}
      <div className="space-y-4">
        {stats.send && (
          <div className="flex justify-between items-center">
            <span className="text-sm uppercase text-irohGray-500">Send</span>
            <span className="font-spaceMono">{stats.send}</span>
          </div>
        )}
        
        {/* Active progress bar or recv stats */}
        <div className="space-y-2">
          <div className="flex justify-between items-center">
            <span className="text-sm uppercase text-irohGray-500">Recv</span>
            {stats.recv ? (
              <span className="font-spaceMono">{stats.recv}</span>
            ) : null}
          </div>
          {!isComplete && length > 0 && message === 'recv' && (
            <ProgressBar
              message={message}
              position={position}
              length={length}
            />
          )}
        </div>

        {/* Echo stats */}
        {stats.echo && (
          <div className="flex justify-between items-center">
            <span className="text-sm uppercase text-irohGray-500">Echo</span>
            <span className="font-spaceMono">{stats.echo}</span>
          </div>
        )}
      </div>
      
      {/* Active progress bar for send/echo */}
      {!isComplete && length > 0 && message !== 'recv' && (
        <div className="mt-4">
          <ProgressBar
            message={message}
            position={position}
            length={length}
          />
        </div>
      )}
    </div>
  );
}; 