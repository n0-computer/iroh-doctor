import './styles.css'

function App() {
  return (
    <div className="min-h-screen bg-gray-100 p-8">
      <div className="max-w-md mx-auto">
        <h1 className="text-3xl font-bold text-center mb-8 text-gray-800">
          iroh doctor
        </h1>
        
        <div className="space-y-4">
          <button 
            className="w-full bg-blue-500 hover:bg-blue-600 text-white font-semibold py-3 px-4 rounded-lg transition-colors"
            onClick={() => {/* TODO: Implement accept connections */}}
          >
            Accept Connections
          </button>
          
          <button 
            className="w-full bg-green-500 hover:bg-green-600 text-white font-semibold py-3 px-4 rounded-lg transition-colors"
            onClick={() => {/* TODO: Implement connect */}}
          >
            Connect
          </button>
        </div>
      </div>
    </div>
  )
}

export default App
