import { useState } from 'react'
import './styles.css'
import { ConnectionScreen } from './components/ConnectionScreen'
import { AcceptedConnScreen } from './components/AcceptedConnScreen'

function App() {
  const [screen, setScreen] = useState<'home' | 'accepting' | 'connecting'>('home')
  
  // This would come from your backend in reality
  const mockConnectionString = 'iroh-doctor connect uvpsmezolzb55a2nknbtf5tkedkibq7fbij2gevaogw6uzljtapa'

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
              onClick={() => setScreen('accepting')}
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
            connectionString={mockConnectionString}
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
