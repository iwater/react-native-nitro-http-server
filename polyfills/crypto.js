module.exports = {
  createHmac: (algorithm, key) => {
    return {
      update: (data) => {
        return {
          digest: (encoding) => {
            return data;
          }
        }
      }
    }
  }
};