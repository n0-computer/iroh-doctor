import React, { useState, useCallback, useEffect } from 'react';
import { ProgressBar, ProgressBarWrapper } from './ProgressBar';

const TOTAL_BYTES = 1024 * 1024 * 16; // 16MB
const UPDATE_INTERVAL = 50; // ms

interface Stats {
  send: string;
  recv: string;
  echo: string;
}

export const AcceptedConnScreen: React.FC = () => {
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
    <div className="p-4">
      <h2 className="text-xl font-bold mb-4">Connection Accepted</h2>
      
      {/* Stats display */}
      <div className="mb-4 space-y-2">
        {stats.send && <div>Send: {stats.send}</div>}
        {stats.recv && <div>Recv: {stats.recv}</div>}
        {stats.echo && <div>Echo: {stats.echo}</div>}
      </div>
      
      {/* Active progress bar */}
      {length > 0 && (
        <ProgressBar
          message={message}
          position={position}
          length={length}
        />
      )}
    </div>
  );
}; 