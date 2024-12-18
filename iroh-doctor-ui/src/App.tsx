import { useState, useEffect } from 'react'
import { listen } from '@tauri-apps/api/event'
import './styles.css'
import { AcceptingConnScreen } from './components/AcceptingConnScreen'
import { ConnectingScreen } from './components/ConnectingScreen'
import { startAcceptingConnections } from './bindings'
import { QrScannerScreen } from './components/QrScannerScreen'

function App() {
  const [screen, setScreen] = useState<'home' | 'accepting' | 'connecting' | 'scanning'>('home')
  const [connectionString, setConnectionString] = useState<string>('')
  
  useEffect(() => {
    // Listen for connection accepted event
    const unlisten = listen('connection-accepted', () => {
      setScreen('connecting');
    });

    return () => {
      unlisten.then(fn => fn());
    };
  }, []);

  const handleAcceptConnections = async () => {
    try {
      const connString = await startAcceptingConnections();
      setConnectionString(connString);
      setScreen('accepting');
    } catch (err) {
      console.error('Failed to start accepting connections:', err);
      // TODO: Show error to user
    }
  };

  return (
    <div className="min-h-screen flex flex-col items-center justify-start p-8 font-space"
      style={screen === 'scanning' ? {
        backgroundColor: 'rgba(255, 255, 255, 0.8)',
        clipPath: `polygon(
          /* Outer rectangle (the full one) */
          0 0, 0 100%, 100% 100%, 100% 0,
          /* Back to the top left corner */
          0 0,
          /* Inner rectangle (the cutout) */
          10% 25%, 90% 25%, 90% 75%, 10% 75%,
          /* Back to the top left corner of the inner rectangle */
          10% 25%
        )`
      } : {
        backgroundColor: 'white',
      }}>
      <div className="w-full max-w-md">
        <h1 className="text-[2.5rem] leading-tight font-koulen text-center mb-12 text-irohGray-900">
          iroh doctor
        </h1>
        
        {screen === 'home' ? (
          <div className="flex-col space-y-3">
            <button 
              className="w-full my-4 p-3 px-4 transition bg-irohGray-800 text-irohPurple-500 uppercase hover:bg-irohGray-700 hover:text-gray-200 font-medium"
              onClick={handleAcceptConnections}
            >
              Accept Connections
            </button>
            
            <button 
              className="w-full my-4 p-3 px-4 transition bg-white text-irohGray-800 uppercase hover:bg-irohGray-100 border border-irohGray-200 font-medium"
              onClick={() => setScreen('connecting')}
            >
              Connect
            </button>
          </div>
        ) : screen === 'accepting' ? (
          <AcceptingConnScreen 
            connectionString={connectionString}
            onBack={() => setScreen('home')}
          />
        ) : screen === 'connecting' ? (
          <ConnectingScreen 
            onBack={() => setScreen('home')} 
            onScanClick={() => setScreen('scanning')}
          />
        ) : (
          <QrScannerScreen 
            onBack={() => setScreen('connecting')}
            onScan={(nodeId) => {
              // TODO: Implement connection logic
              console.log('Scanned NodeId:', nodeId);
              setScreen('connecting');
            }}
          />
        )}
      </div>
    </div>
  )
}

export default App
