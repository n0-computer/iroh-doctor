import { useState } from 'react'
import './styles.css'
import { ConnectionScreen } from './components/ConnectionScreen'
import { AcceptedConnScreen } from './components/AcceptedConnScreen'
import { startAcceptingConnections } from './bindings'

function App() {
  const [screen, setScreen] = useState<'home' | 'accepting' | 'connecting'>('home')
  const [connectionString, setConnectionString] = useState<string>('')
  
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
    <div className="min-h-screen bg-white flex flex-col items-center justify-start p-8 font-space">
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
          <ConnectionScreen 
            connectionString={connectionString}
            onBack={() => setScreen('home')}
          />
        ) : (
          <AcceptedConnScreen onBack={() => setScreen('home')} />
        )}
      </div>
    </div>
  )
}

export default App
