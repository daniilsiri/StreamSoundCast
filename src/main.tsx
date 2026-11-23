import { createRoot } from 'react-dom/client'
import '@fontsource-variable/manrope'
import './styles.css'
import { App } from './App'
import { initialize } from './state'

createRoot(document.getElementById('root')!).render(<App />)
void initialize()
