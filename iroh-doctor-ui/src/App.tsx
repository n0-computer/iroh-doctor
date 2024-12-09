import './styles.css'

function App() {
  return (
    <div className="min-h-screen bg-white flex flex-col items-center justify-start p-8 font-space">
      <div className="w-full max-w-md">
        <h1 className="text-[2.5rem] leading-tight font-koulen text-center mb-12 text-irohGray-900">
          iroh doctor
        </h1>
        
        <div className="flex-col space-y-3">
          <button 
            className="w-full bg-irohGray-900 hover:bg-irohGray-800 text-white text-sm font-medium py-2.5 px-5 rounded transition-all duration-200 border border-irohGray-800 hover:border-irohGray-700 flex items-center justify-center"
            onClick={() => {/* TODO: Implement accept connections */}}
          >
            Accept Connections
          </button>
          
          <button 
            className="w-full bg-irohGray-100 hover:bg-irohGray-200 text-irohGray-900 text-sm font-medium py-2.5 px-5 rounded transition-all duration-200 border border-irohGray-200 hover:border-irohGray-300 flex items-center justify-center"
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
