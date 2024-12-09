import React, { useState, useCallback, useEffect } from 'react';
import { ProgressBar, ProgressBarWrapper } from './ProgressBar';

const TOTAL_BYTES = 1024 * 1024 * 16; // 16MB
const UPDATE_INTERVAL = 50; // ms

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
  
  // Create a progress bar wrapper that can be passed to Rust
  const progressBar = useCallback(() => {
    return new ProgressBarWrapper(setMessage, setPosition, setLength);
  }, []);

  // Simulate a single test (send, recv, or echo)
  const simulateTest = async (testName: string): Promise<void> => {
    setMessage(testName);
    setLength(TOTAL_BYTES);
    setPosition(0);

    return new Promise((resolve) => {
      let currentPosition = 0;
      const increment = TOTAL_BYTES / 20; // Complete in 20 steps
      
      const interval = setInterval(() => {
        currentPosition = Math.min(currentPosition + increment, TOTAL_BYTES);
        setPosition(currentPosition);
        
        if (currentPosition >= TOTAL_BYTES) {
          clearInterval(interval);
          // Calculate and display speed
          const speed = `${(16).toFixed(2)} MiB/s`;
          setStats(prev => ({ ...prev, [testName]: speed }));
          setLength(0); // Hide progress bar
          resolve();
        }
      }, UPDATE_INTERVAL);
    });
  };

  // Run all tests in sequence
  useEffect(() => {
    const runTests = async () => {
      await simulateTest('send');
      await simulateTest('recv');
      await simulateTest('echo');
    };
    
    runTests();
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
          {length > 0 && message === 'recv' && (
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
      {length > 0 && message !== 'recv' && (
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