use crate::SecretError;
use keyring::Entry;

pub async fn set(service: &str, account: &str, password: &str) -> Result<(), SecretError> {
    let entry = Entry::new(service, account)
        .map_err(|e| SecretError::System(e.to_string()))?;
    
    entry.set_password(password)
        .map_err(|e| SecretError::System(e.to_string()))
}

pub async fn get(service: &str, account: &str) -> Result<String, SecretError> {
    let entry = Entry::new(service, account)
        .map_err(|e| SecretError::System(e.to_string()))?;
        
    match entry.get_password() {
        Ok(pwd) => Ok(pwd),
        Err(keyring::Error::NoEntry) => Err(SecretError::NotFound),
        Err(e) => Err(SecretError::System(e.to_string())),
    }
}

pub async fn delete(service: &str, account: &str) -> Result<(), SecretError> {
    let entry = Entry::new(service, account)
        .map_err(|e| SecretError::System(e.to_string()))?;
        
    match entry.delete_password() {
        Ok(_) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(SecretError::System(e.to_string())),
    }
}
